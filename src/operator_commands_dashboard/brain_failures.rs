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
                let confidence_opt = j.get("confidence").and_then(|v| v.as_f64());
                let confidence = confidence_opt.unwrap_or(0.0);

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
                    fallback_description(phase, decision, confidence_opt)
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
                    "rationale": humanize_rationale(rationale),
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

/// Humanize a raw machine decision token (e.g. `consolidate_memory`) into
/// plain words (`consolidate memory`) for operator display. P3 of #2358 —
/// keeps bare snake/kebab identifiers out of the human-facing Brain Failures
/// description. Returns an empty string for an empty/whitespace input.
fn humanize_decision(raw: &str) -> String {
    let d = raw.trim();
    if d.is_empty() {
        return String::new();
    }
    d.replace(['_', '-'], " ")
}

/// Build the plain-English Brain Failures description for a deterministic
/// (non-parse) fallback. P3 of #2358 — no machine jargon (`deterministic
/// fallback`, snake_case decision tokens), and the confidence figure is only
/// shown when it was actually recorded so an absent value never reads as a
/// real "0%".
fn fallback_description(phase: &str, decision: &str, confidence: Option<f64>) -> String {
    let decision_human = humanize_decision(decision);
    let chose = if decision_human.is_empty() {
        String::new()
    } else {
        format!(" It chose to {decision_human}.")
    };
    let conf = match confidence {
        Some(c) => format!(
            " The rules were {:.0}% confident in that choice.",
            c * 100.0
        ),
        None => String::new(),
    };
    format!(
        "The {phase} phase used its built-in safety rules instead of the language model.{chose}{conf}"
    )
}

/// Humanize a raw brain `rationale` marker for operator display. P3 of #2358 —
/// the canonical rationale strings (set in `ooda_reasoners`) are insider shorthand
/// like `deterministic-brain: prefix-routed`; this maps them to plain English
/// at the display layer only. The persisted/canonical rationale is unchanged so
/// logs and `ooda_reasoners` tests stay green. Unknown rationales are passed
/// through with the machine `<x>-brain:` prefix stripped and common shorthand
/// expanded, so no raw token reaches the operator.
fn humanize_rationale(raw: &str) -> String {
    let r = raw.trim();
    if r.is_empty() {
        return String::new();
    }
    match r {
        "deterministic-brain: prefix-routed" | "fallback-brain: prefix-routed" => {
            "Chosen by Simard's built-in routing rules (no language model was needed).".to_string()
        }
        "deterministic-brain: no LLM configured" => {
            "Used the built-in rules because no language model is configured.".to_string()
        }
        other => {
            let body = other
                .split_once("-brain:")
                .map(|(_, rest)| rest.trim())
                .unwrap_or(other);
            body.replace("prefix-routed", "chosen by built-in routing rules")
                .replace("no LLM configured", "no language model configured")
                .trim()
                .to_string()
        }
    }
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

    #[test]
    fn humanize_decision_spaces_machine_tokens() {
        assert_eq!(
            humanize_decision("consolidate_memory"),
            "consolidate memory"
        );
        assert_eq!(humanize_decision("run-improvement"), "run improvement");
        assert_eq!(humanize_decision("  "), "");
        assert_eq!(humanize_decision(""), "");
        // No machine separators -> unchanged.
        assert_eq!(humanize_decision("continue"), "continue");
    }

    #[test]
    fn humanize_rationale_maps_known_markers() {
        assert_eq!(
            humanize_rationale("deterministic-brain: prefix-routed"),
            "Chosen by Simard's built-in routing rules (no language model was needed)."
        );
        assert_eq!(
            humanize_rationale("fallback-brain: prefix-routed"),
            "Chosen by Simard's built-in routing rules (no language model was needed)."
        );
        assert_eq!(
            humanize_rationale("deterministic-brain: no LLM configured"),
            "Used the built-in rules because no language model is configured."
        );
    }

    #[test]
    fn humanize_rationale_strips_brain_prefix_and_passes_prose() {
        // Unknown "<x>-brain:" markers get the machine prefix stripped and
        // shorthand expanded; no raw "*-brain:" / "prefix-routed" token leaks.
        let out = humanize_rationale("orient-brain: prefix-routed");
        assert!(!out.contains("-brain:"), "leaked machine prefix: {out}");
        assert!(!out.contains("prefix-routed"), "leaked shorthand: {out}");
        // Plain-English rationale (already human) passes through unchanged.
        assert_eq!(
            humanize_rationale("llm-brain: high-leverage progress"),
            "high-leverage progress"
        );
        assert_eq!(humanize_rationale(""), "");
    }

    #[test]
    fn fallback_description_is_plain_and_jargon_free() {
        let d = fallback_description("decide", "consolidate_memory", Some(0.5));
        assert_eq!(
            d,
            "The decide phase used its built-in safety rules instead of the language model. \
             It chose to consolidate memory. The rules were 50% confident in that choice."
        );
        // No machine jargon survives.
        for banned in [
            "deterministic fallback",
            "language model brain",
            "consolidate_memory",
        ] {
            assert!(
                !d.contains(banned),
                "description leaked jargon {banned:?}: {d}"
            );
        }
    }

    #[test]
    fn fallback_description_omits_absent_confidence_and_empty_decision() {
        // Absent confidence must NOT render as a real "0%".
        let d = fallback_description("orient", "", None);
        assert_eq!(
            d,
            "The orient phase used its built-in safety rules instead of the language model."
        );
        assert!(
            !d.contains('%'),
            "absent confidence must not show a percentage: {d}"
        );
        assert!(
            !d.contains("It chose to"),
            "empty decision must omit the choice clause: {d}"
        );
    }
}
