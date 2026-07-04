//! Thinking-tab display honesty (issue #2580).
//!
//! The Thinking tab renders one entry per OODA cycle. When both active goals
//! already have healthy engineers, the Decide reasoner *correctly* emits a
//! no-action deferral every cycle ("goal already has a live, healthy engineer"
//! / "already assigned to subordinate"). Rendered verbatim, cycle after cycle,
//! this looks like an infinite loop even though the underlying work is
//! advancing inside the engineers.
//!
//! This module post-processes the raw cycle reports at the **display layer**
//! only — it does not touch the reasoner. It:
//!
//!   * classifies each cycle as `deferring` (a deliberate no-action deferral to
//!     an already-active engineer), `progressing` (launched work / produced an
//!     artifact), or `reasoning` (anything else);
//!   * collapses a consecutive run of identical deferrals into a single entry
//!     carrying a `repeat_count` and the goals being deferred on, so the
//!     timeline shows "deferring to an active engineer on <goal> (xN)" instead
//!     of N identical lines; and
//!   * flags `loop_suspected` only when a **non-progressing, non-deferral**
//!     decision repeats `LOOP_REPEAT_THRESHOLD` times — a genuine stuck loop,
//!     not a healthy deferral.

use serde_json::{Value, json};

/// A `reasoning` (non-progress, non-deferral) decision must repeat at least
/// this many times in a row before it is flagged as a suspected loop.
const LOOP_REPEAT_THRESHOLD: u64 = 3;

/// Substrings (matched case-insensitively) that mark a cycle's action as a
/// deliberate no-action deferral to an already-active, healthy engineer. These
/// are the real skip-path strings emitted by the dispatch/decide paths
/// (`spawn.rs`, `concurrent.rs`) plus the reasoner's healthy-engineer phrasing.
const DEFERRAL_MARKERS: &[&str] = &[
    "already assigned to subordinate",
    "already has a subordinate",
    "already has a live, healthy engineer",
    "already has a live",
    "live, healthy engineer",
    "healthy engineer",
];

/// Substrings (case-insensitive) that mark a cycle as making forward progress —
/// launching an engineer or producing an artifact.
const PROGRESS_MARKERS: &[&str] = &["pr #", "commit", "launched", "dispatched"];

/// Disposition of one thinking cycle for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// A deliberate, correct no-action deferral to an already-active engineer.
    Deferring,
    /// The cycle launched work or produced an artifact.
    Progressing,
    /// Any other reasoning cycle (candidate for loop detection if it repeats).
    Reasoning,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deferring => "deferring",
            Self::Progressing => "progressing",
            Self::Reasoning => "reasoning",
        }
    }
}

/// Recursively collect every string value stored under a `"goal_id"` key.
fn collect_goal_ids(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "goal_id"
                    && let Some(id) = v.as_str()
                    && !id.is_empty()
                    && !out.iter().any(|g| g == id)
                {
                    out.push(id.to_string());
                }
                collect_goal_ids(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_goal_ids(v, out);
            }
        }
        _ => {}
    }
}

/// Lower-cased haystack of the report's human-readable decision text: the
/// summary plus every outcome `detail` string.
fn decision_haystack(report: &Value) -> String {
    let mut s = String::new();
    if let Some(summary) = report.get("summary").and_then(|v| v.as_str()) {
        s.push_str(summary);
        s.push(' ');
    }
    if let Some(outcomes) = report.get("outcomes").and_then(|v| v.as_array()) {
        for o in outcomes {
            for key in ["detail", "action_description", "action_kind"] {
                if let Some(t) = o.get(key).and_then(|v| v.as_str()) {
                    s.push_str(t);
                    s.push(' ');
                }
            }
        }
    }
    s.to_ascii_lowercase()
}

/// True when any outcome shows a live spawned engineer this cycle.
fn has_live_spawn(report: &Value) -> bool {
    report
        .get("outcomes")
        .and_then(|v| v.as_array())
        .is_some_and(|outcomes| {
            outcomes.iter().any(|o| {
                o.get("spawn_engineer")
                    .and_then(|se| se.get("status"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("live"))
            })
        })
}

/// Classify a single cycle report and return its disposition plus the goal ids
/// it concerns (sorted, de-duplicated).
pub(crate) fn classify_cycle(report: &Value) -> (Disposition, Vec<String>) {
    let mut goals = Vec::new();
    collect_goal_ids(report, &mut goals);
    goals.sort();

    let haystack = decision_haystack(report);
    let disposition = if DEFERRAL_MARKERS.iter().any(|m| haystack.contains(m)) {
        Disposition::Deferring
    } else if has_live_spawn(report) || PROGRESS_MARKERS.iter().any(|m| haystack.contains(m)) {
        Disposition::Progressing
    } else {
        Disposition::Reasoning
    };
    (disposition, goals)
}

/// Grouping key for collapsing consecutive cycles. Progressing cycles never
/// collapse (each is distinct forward progress); deferrals collapse by the goal
/// set they defer on; reasoning cycles collapse by their normalized text so an
/// identical repeated decision can be counted (and flagged as a loop).
fn group_key(report: &Value, disposition: Disposition, goals: &[String]) -> String {
    match disposition {
        Disposition::Deferring => format!("defer:{}", goals.join(",")),
        Disposition::Progressing => format!(
            "progress:{}",
            report
                .get("cycle_number")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        ),
        Disposition::Reasoning => format!("reason:{}", decision_haystack(report).trim()),
    }
}

fn cycle_number(report: &Value) -> u64 {
    report
        .get("cycle_number")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// Collapse a chronologically-ordered (most-recent-first) list of cycle reports
/// for honest display. Consecutive identical deferrals become a single entry
/// with a `repeat_count`; a repeated non-progress reasoning decision is flagged
/// `loop_suspected`. Progressing and one-off cycles pass through annotated with
/// their disposition.
pub(crate) fn collapse_reports(reports: Vec<Value>) -> Vec<Value> {
    let classified: Vec<(Disposition, Vec<String>, String, Value)> = reports
        .into_iter()
        .map(|r| {
            let (disp, goals) = classify_cycle(&r);
            let key = group_key(&r, disp, &goals);
            (disp, goals, key, r)
        })
        .collect();

    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < classified.len() {
        let (disp, ref goals, ref key, _) = classified[i];
        // Extend the run while the next entry shares the same disposition + key.
        let mut j = i + 1;
        while j < classified.len() && classified[j].0 == disp && &classified[j].2 == key {
            j += 1;
        }
        let repeat_count = (j - i) as u64;
        let representative = classified[i].3.clone();
        let cycle_first = cycle_number(&representative);
        let cycle_last = cycle_number(&classified[j - 1].3);

        let mut entry = representative;
        if let Value::Object(map) = &mut entry {
            map.insert("disposition".to_string(), json!(disp.as_str()));
            map.insert("repeat_count".to_string(), json!(repeat_count));
            map.insert("cycle_number_first".to_string(), json!(cycle_first));
            map.insert("cycle_number_last".to_string(), json!(cycle_last));
            match disp {
                Disposition::Deferring => {
                    map.insert("deferring_to".to_string(), json!(goals));
                    let goal_txt = if goals.is_empty() {
                        "an active engineer".to_string()
                    } else {
                        format!("an active engineer on {}", goals.join(", "))
                    };
                    let times = if repeat_count == 1 {
                        String::new()
                    } else {
                        format!(" (repeated {repeat_count} cycles)")
                    };
                    map.insert(
                        "collapsed_summary".to_string(),
                        json!(format!("Deferring to {goal_txt}{times}")),
                    );
                }
                Disposition::Reasoning if repeat_count >= LOOP_REPEAT_THRESHOLD => {
                    map.insert("loop_suspected".to_string(), json!(true));
                }
                _ => {}
            }
        }
        out.push(entry);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deferral_report(cycle: u64, goal: &str) -> Value {
        json!({
            "cycle_number": cycle,
            "summary": format!("cycle {cycle}"),
            "outcomes": [{
                "action_kind": "AdvanceGoal",
                "success": true,
                "goal_id": goal,
                "detail": "no_action - goal already has a live, healthy engineer",
            }],
        })
    }

    fn progress_report(cycle: u64, goal: &str) -> Value {
        json!({
            "cycle_number": cycle,
            "summary": format!("cycle {cycle}"),
            "outcomes": [{
                "action_kind": "AdvanceGoal",
                "success": true,
                "goal_id": goal,
                "detail": "launched sub-agent, opened PR #42",
                "spawn_engineer": {"status": "live", "goal_id": goal},
            }],
        })
    }

    fn reasoning_report(cycle: u64, text: &str) -> Value {
        json!({
            "cycle_number": cycle,
            "summary": text,
            "outcomes": [{
                "action_kind": "Reflect",
                "success": true,
                "detail": text,
            }],
        })
    }

    #[test]
    fn classify_detects_deferral_progress_and_reasoning() {
        assert_eq!(
            classify_cycle(&deferral_report(5, "g1")).0,
            Disposition::Deferring
        );
        assert_eq!(
            classify_cycle(&progress_report(6, "g1")).0,
            Disposition::Progressing
        );
        assert_eq!(
            classify_cycle(&reasoning_report(7, "considered the backlog")).0,
            Disposition::Reasoning
        );
    }

    #[test]
    fn classify_uses_real_skip_path_strings() {
        let r = json!({
            "cycle_number": 1,
            "outcomes": [{"detail": "spawn_engineer skipped: goal 'g1' already assigned to subordinate 'engineer-x'"}],
        });
        assert_eq!(classify_cycle(&r).0, Disposition::Deferring);
    }

    #[test]
    fn collapses_consecutive_deferrals_with_repeat_count() {
        let reports = vec![
            deferral_report(10, "g1"),
            deferral_report(9, "g1"),
            deferral_report(8, "g1"),
        ];
        let out = collapse_reports(reports);
        assert_eq!(
            out.len(),
            1,
            "three identical deferrals collapse to one entry"
        );
        assert_eq!(out[0]["repeat_count"], 3);
        assert_eq!(out[0]["disposition"], "deferring");
        assert_eq!(out[0]["cycle_number_first"], 10);
        assert_eq!(out[0]["cycle_number_last"], 8);
        assert_eq!(out[0]["deferring_to"], json!(["g1"]));
        assert!(
            out[0]["collapsed_summary"]
                .as_str()
                .unwrap()
                .contains("Deferring to an active engineer on g1")
        );
        // A healthy deferral is NEVER flagged as a loop.
        assert!(out[0].get("loop_suspected").is_none());
    }

    #[test]
    fn different_goal_deferrals_do_not_collapse_together() {
        let reports = vec![deferral_report(10, "g1"), deferral_report(9, "g2")];
        let out = collapse_reports(reports);
        assert_eq!(out.len(), 2, "deferrals on different goals stay separate");
    }

    #[test]
    fn progress_cycles_are_shown_individually() {
        let reports = vec![progress_report(10, "g1"), progress_report(9, "g1")];
        let out = collapse_reports(reports);
        assert_eq!(out.len(), 2, "forward-progress cycles are never collapsed");
        assert_eq!(out[0]["disposition"], "progressing");
    }

    #[test]
    fn repeated_reasoning_is_flagged_as_loop() {
        let reports = vec![
            reasoning_report(5, "stuck on the same thing"),
            reasoning_report(4, "stuck on the same thing"),
            reasoning_report(3, "stuck on the same thing"),
        ];
        let out = collapse_reports(reports);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["repeat_count"], 3);
        assert_eq!(out[0]["loop_suspected"], true);
    }

    #[test]
    fn two_repeats_below_threshold_are_not_a_loop() {
        let reports = vec![
            reasoning_report(5, "thinking"),
            reasoning_report(4, "thinking"),
        ];
        let out = collapse_reports(reports);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["repeat_count"], 2);
        assert!(out[0].get("loop_suspected").is_none());
    }
}
