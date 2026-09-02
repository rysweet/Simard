//! `/api/brain-failures` endpoint — surfaces when and how the OODA brain
//! failed so the operator can see Simard's self-awareness gaps (issue #2043).
//!
//! Data sources:
//!   1. `~/.simard/cycle_reports/cycle_*.json` — `brain_judgments[]` entries
//!      where `fallback == true` or `parse_failure != null`.
//!   2. `~/.simard/metrics/metrics.jsonl` — `brain_parse_error` /
//!      `brain_parse_failure` metric entries for a quick summary count.
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
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::routes::resolve_state_root;

/// Maximum number of cycle reports to scan (most recent first).
const MAX_CYCLES_TO_SCAN: usize = 50;

/// Maximum number of failure entries to return.
const MAX_FAILURES_RETURNED: usize = 200;

/// Rolling window (minutes) for the *current* Brain-Failures rate. The tab
/// headline reports failures in this window plus a per-hour rate, so a scary
/// cumulative lifetime total can never masquerade as the current state
/// (issue #2580). Zero failures in the window renders green.
const RECENT_WINDOW_MINUTES: i64 = 60;

/// Metric names that count as a genuine, post-sanitization brain parse failure.
/// `brain_parse_error` is the companion workstream's explicit metric that fires
/// only on a real parse failure after bounded retry; `brain_parse_failure` is
/// the pre-existing name. Both are read so the recent count stays truthful
/// across the transition.
const BRAIN_PARSE_METRIC_NAMES: &[&str] = &["brain_parse_error", "brain_parse_failure"];

pub(crate) async fn brain_failures() -> Json<Value> {
    let state_root = resolve_state_root();
    let cycle_dir = state_root.join("cycle_reports");
    let metrics_path = state_root.join("metrics").join("metrics.jsonl");

    let mut failures: Vec<Value> = Vec::new();
    let mut total_fallback_count: u64 = 0;
    let mut total_parse_failure_count: u64 = 0;
    let mut cycles_scanned: u32 = 0;
    // Timestamps of deterministic fallbacks seen in cycle reports, used to
    // compute the *recent* (bounded-window) count. Parse failures are counted
    // from the timestamped metric instead (authoritative, see below).
    let mut fallback_instants: Vec<DateTime<Utc>> = Vec::new();

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
                    if let Some(at) = parse_rfc3339(cycle_timestamp) {
                        fallback_instants.push(at);
                    }
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

    // ── Current (bounded-window) view (issue #2580) ─────────────────────────
    // Show what is happening NOW, not an accumulated lifetime number. Parse
    // failures are counted from the timestamped brain parse-error metric (the
    // honest, post-sanitization signal); deterministic fallbacks from the
    // cycle-report timestamps gathered above. Zero in the window ⇒ green.
    let now = Utc::now();
    let since = now - Duration::minutes(RECENT_WINDOW_MINUTES);
    let recent_fallback = count_at_or_after(&fallback_instants, since);
    let recent_parse_failure = recent_parse_failures_from_metrics(&metrics_path, since);
    let recent_total = recent_fallback + recent_parse_failure;
    let recent_rate_per_hour = rate_per_hour(recent_total, RECENT_WINDOW_MINUTES);
    let recent_status = recent_status(recent_total, recent_parse_failure);

    Json(json!({
        "failures": failures,
        // Current state: a bounded, honest window separate from any lifetime
        // total. The dashboard headline reads from here.
        "recent": {
            "window_minutes": RECENT_WINDOW_MINUTES,
            "total": recent_total,
            "fallback": recent_fallback,
            "parse_failure": recent_parse_failure,
            "rate_per_hour": recent_rate_per_hour,
            "status": recent_status,
        },
        // Lifetime cumulative counts — clearly labelled so they are never
        // mistaken for the current rate.
        "lifetime": {
            "parse_failure_count": metrics_parse_failure_count,
        },
        "summary": {
            "total_fallback_count": total_fallback_count,
            "total_parse_failure_count": total_parse_failure_count,
            "metrics_parse_failure_count": metrics_parse_failure_count,
            "cycles_scanned": cycles_scanned,
        },
        "timestamp": now.to_rfc3339(),
    }))
}

/// Parse an RFC3339 timestamp string into a UTC instant, returning `None` for
/// an empty or unparseable value (so it is conservatively excluded from the
/// recent-window count rather than inflating it).
fn parse_rfc3339(ts: &str) -> Option<DateTime<Utc>> {
    if ts.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Count instants at or after `since`.
fn count_at_or_after(instants: &[DateTime<Utc>], since: DateTime<Utc>) -> u64 {
    instants.iter().filter(|t| **t >= since).count() as u64
}

/// Failures-per-hour for `count` events observed over a `window_minutes`
/// window. Returns `0.0` for a zero or non-positive window.
fn rate_per_hour(count: u64, window_minutes: i64) -> f64 {
    if window_minutes <= 0 {
        return 0.0;
    }
    count as f64 / (window_minutes as f64 / 60.0)
}

/// Traffic-light status for the current window: `ok` (green) when there are no
/// failures at all, `err` (red) when any genuine parse failure occurred, and
/// `warn` (amber) for deterministic fallbacks only. Zero ⇒ green, never a
/// stale large number.
fn recent_status(recent_total: u64, recent_parse_failure: u64) -> &'static str {
    if recent_total == 0 {
        "ok"
    } else if recent_parse_failure > 0 {
        "err"
    } else {
        "warn"
    }
}

/// Count genuine brain parse failures recorded at or after `since`, read from
/// the timestamped metric log ([`BRAIN_PARSE_METRIC_NAMES`]). This is the
/// authoritative, honest recent signal — it never returns a lifetime total.
fn recent_parse_failures_from_metrics(path: &std::path::Path, since: DateTime<Utc>) -> u64 {
    use crate::self_metrics::MetricEntry;
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<MetricEntry>(line).ok())
        .filter(|e| BRAIN_PARSE_METRIC_NAMES.contains(&e.metric_name.as_str()))
        .filter(|e| e.timestamp >= since)
        .count() as u64
}

/// Count lifetime brain parse failures in `metrics.jsonl`, keyed on the parsed
/// `metric_name` field against [`BRAIN_PARSE_METRIC_NAMES`]. This is the
/// time-unbounded companion to [`recent_parse_failures_from_metrics`]: both
/// read the SAME metric-name set so the lifetime total and the recent window
/// stay consistent across the `brain_parse_failure` → `brain_parse_error`
/// metric-name transition (#4187).
///
/// Keying on the structured `metric_name` (rather than a raw substring match on
/// the whole JSON line) fixes two defects: it no longer drops genuine
/// `brain_parse_error` entries the substring `"brain_parse_failure"` never
/// matched, and it no longer counts an unrelated metric whose `context` field
/// merely mentions the string.
fn count_brain_parse_failure_metrics(path: &std::path::Path) -> u64 {
    use crate::self_metrics::MetricEntry;
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<MetricEntry>(line).ok())
        .filter(|e| BRAIN_PARSE_METRIC_NAMES.contains(&e.metric_name.as_str()))
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
/// the canonical rationale strings (set in `ooda_brain`) are insider shorthand
/// like `deterministic-brain: prefix-routed`; this maps them to plain English
/// at the display layer only. The persisted/canonical rationale is unchanged so
/// logs and `ooda_brain` tests stay green. Unknown rationales are passed
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
            r#"{"timestamp":"2026-05-19T05:23:26Z","metric_name":"ooda_cycle","value":1.0,"context":""}
{"timestamp":"2026-05-19T05:24:26Z","metric_name":"brain_parse_failure","value":1.0,"context":""}
{"timestamp":"2026-05-19T05:25:26Z","metric_name":"brain_parse_failure","value":1.0,"context":""}
{"timestamp":"2026-05-19T05:26:26Z","metric_name":"other_metric","value":42.0,"context":""}
"#,
        )
        .unwrap();
        assert_eq!(count_brain_parse_failure_metrics(&path), 2);
    }

    /// #4187: the lifetime count must also include the post-transition
    /// `brain_parse_error` metric name, mirroring the recent-window path, so the
    /// lifetime total never silently reads 0 while the recent window shows real
    /// failures.
    #[test]
    fn count_brain_parse_failure_metrics_counts_both_metric_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-05-19T05:24:26Z","metric_name":"brain_parse_failure","value":1.0,"context":""}
{"timestamp":"2026-05-19T05:25:26Z","metric_name":"brain_parse_error","value":1.0,"context":""}
{"timestamp":"2026-05-19T05:26:26Z","metric_name":"brain_parse_error","value":1.0,"context":""}
"#,
        )
        .unwrap();
        assert_eq!(count_brain_parse_failure_metrics(&path), 3);
    }

    /// #4187: keying on the structured `metric_name` must NOT count an unrelated
    /// metric whose `context` merely mentions the parse-failure string.
    #[test]
    fn count_brain_parse_failure_metrics_ignores_context_false_positive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-05-19T05:24:26Z","metric_name":"unrelated_metric","value":1.0,"context":"observed brain_parse_failure downstream"}
{"timestamp":"2026-05-19T05:25:26Z","metric_name":"brain_parse_failure","value":1.0,"context":""}
"#,
        )
        .unwrap();
        assert_eq!(count_brain_parse_failure_metrics(&path), 1);
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

    // ── recent (bounded-window) telemetry honesty (issue #2580) ─────────────

    #[test]
    fn rate_per_hour_scales_by_window() {
        // 3 events in a 60-minute window ⇒ 3.0/hr.
        assert_eq!(rate_per_hour(3, 60), 3.0);
        // 3 events in a 30-minute window ⇒ 6.0/hr.
        assert_eq!(rate_per_hour(3, 30), 6.0);
        // Zero events ⇒ zero rate; degenerate window ⇒ zero (no divide-by-zero).
        assert_eq!(rate_per_hour(0, 60), 0.0);
        assert_eq!(rate_per_hour(5, 0), 0.0);
    }

    #[test]
    fn recent_status_is_green_only_when_zero() {
        // The core honesty property: zero current failures ⇒ green, never a
        // large stale number.
        assert_eq!(recent_status(0, 0), "ok");
        assert_eq!(recent_status(2, 0), "warn"); // fallbacks only
        assert_eq!(recent_status(2, 1), "err"); // a real parse failure
    }

    #[test]
    fn count_at_or_after_windows_correctly() {
        let now = Utc::now();
        let instants = vec![
            now - Duration::minutes(5),   // in window
            now - Duration::minutes(59),  // in window
            now - Duration::minutes(120), // out of window
        ];
        let since = now - Duration::minutes(60);
        assert_eq!(count_at_or_after(&instants, since), 2);
    }

    #[test]
    fn parse_rfc3339_rejects_empty_and_garbage() {
        assert!(parse_rfc3339("").is_none());
        assert!(parse_rfc3339("not-a-date").is_none());
        assert!(parse_rfc3339("2026-07-04T16:24:11+00:00").is_some());
    }

    #[test]
    fn recent_parse_failures_from_metrics_counts_only_recent_brain_parse_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        let now = Utc::now();
        let recent = (now - Duration::minutes(10)).to_rfc3339();
        let old = (now - Duration::hours(6)).to_rfc3339();
        // Two recent genuine parse failures (both accepted metric names), one
        // old parse failure (out of window), and an unrelated recent metric.
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            json!({"timestamp": recent, "metric_name": "brain_parse_error", "value": 1.0, "context": ""}),
            json!({"timestamp": recent, "metric_name": "brain_parse_failure", "value": 1.0, "context": ""}),
            json!({"timestamp": old, "metric_name": "brain_parse_failure", "value": 1.0, "context": ""}),
            json!({"timestamp": recent, "metric_name": "ooda_cycle", "value": 1.0, "context": ""}),
        );
        std::fs::write(&path, body).unwrap();

        let since = now - Duration::minutes(RECENT_WINDOW_MINUTES);
        assert_eq!(recent_parse_failures_from_metrics(&path, since), 2);
    }

    #[test]
    fn recent_parse_failures_from_metrics_missing_file_is_zero() {
        let since = Utc::now() - Duration::minutes(RECENT_WINDOW_MINUTES);
        assert_eq!(
            recent_parse_failures_from_metrics(std::path::Path::new("/nonexistent/m.jsonl"), since),
            0
        );
    }
}
