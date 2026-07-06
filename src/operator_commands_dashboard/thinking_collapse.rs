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

/// How aggressively consecutive cycles are merged for display.
///
/// * `Strict` is the original, byte-for-byte behaviour used by the PRESERVED
///   second half (`/api/ooda-thinking`, the OODA reasoning breakdown): reasoning
///   cycles collapse only when their decision text is *identical*.
/// * `Relaxed` is used by the FIRST half (`/api/ooda-cycles`, the Cycle History
///   table): reasoning cycles that differ only in cosmetic digit-runs (cycle
///   number, priority/action/issue counts) collapse together, and every row
///   carries a difference-carrying `collapsed_summary` (issue #21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollapseMode {
    /// Legacy identical-text collapse (second half — do not change).
    Strict,
    /// Digit-masked collapse with difference-carrying summaries (first half).
    Relaxed,
}

/// Replace each maximal run of ASCII digits with a single `#`, so summaries that
/// differ only in cosmetic numbers (cycle number, priority/action/issue counts)
/// hash to the same relaxed group key while genuinely different decision *text*
/// stays distinct.
fn mask_digits(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_digit = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out
}

/// The concrete action/decision text for a cycle, used to build a
/// difference-carrying relaxed summary. Prefers a human `action_description`,
/// then the outcome `detail`, then a planned-action `description`. Returns
/// `None` only when the report carries no action text at all.
fn primary_action_text(report: &Value) -> Option<String> {
    if let Some(outcomes) = report.get("outcomes").and_then(|v| v.as_array()) {
        for key in ["action_description", "detail"] {
            for o in outcomes {
                if let Some(t) = o.get(key).and_then(|v| v.as_str()) {
                    let t = t.trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }
    if let Some(planned) = report.get("planned_actions").and_then(|v| v.as_array()) {
        for a in planned {
            if let Some(t) = a.get("description").and_then(|v| v.as_str()) {
                let t = t.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

/// Fallback difference-carrying summary when a cycle carries no action text, so
/// a relaxed row is never blank.
fn default_relaxed_summary(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Progressing => "made forward progress",
        Disposition::Reasoning => "reasoning cycle (no action selected)",
        Disposition::Deferring => "no-action: deferring to active engineer",
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
/// set they defer on; reasoning cycles collapse by their decision text — verbatim
/// under [`CollapseMode::Strict`], or digit-masked under [`CollapseMode::Relaxed`]
/// so cosmetically-numbered boilerplate ("Cycle #N — M priorities considered …")
/// counts as one repeated decision.
fn group_key(
    report: &Value,
    disposition: Disposition,
    goals: &[String],
    mode: CollapseMode,
) -> String {
    match disposition {
        Disposition::Deferring => format!("defer:{}", goals.join(",")),
        Disposition::Progressing => format!(
            "progress:{}",
            report
                .get("cycle_number")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        ),
        Disposition::Reasoning => {
            let text = decision_haystack(report);
            let text = text.trim();
            match mode {
                CollapseMode::Strict => format!("reason:{text}"),
                CollapseMode::Relaxed => format!("reason:{}", mask_digits(text)),
            }
        }
    }
}

fn cycle_number(report: &Value) -> u64 {
    report
        .get("cycle_number")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// Collapse a chronologically-ordered (most-recent-first) list of cycle reports
/// for honest display, using the legacy [`CollapseMode::Strict`] behaviour.
///
/// This is the exact API the PRESERVED second half (`/api/ooda-thinking`) uses;
/// it remains a byte-for-byte alias of `collapse_reports_with(_, Strict)`.
pub(crate) fn collapse_reports(reports: Vec<Value>) -> Vec<Value> {
    collapse_reports_with(reports, CollapseMode::Strict)
}

/// Collapse consecutive equivalent cycles under the requested [`CollapseMode`].
///
/// Consecutive identical deferrals become a single entry with a `repeat_count`;
/// a repeated non-progress reasoning decision is flagged `loop_suspected`.
/// Progressing and one-off cycles pass through annotated with their disposition.
/// Under [`CollapseMode::Relaxed`] every row additionally carries a non-empty,
/// difference-carrying `collapsed_summary`, and reasoning cycles that differ
/// only in cosmetic digits are merged (issue #21).
pub(crate) fn collapse_reports_with(reports: Vec<Value>, mode: CollapseMode) -> Vec<Value> {
    let classified: Vec<(Disposition, Vec<String>, String, Value)> = reports
        .into_iter()
        .map(|r| {
            let (disp, goals) = classify_cycle(&r);
            let key = group_key(&r, disp, &goals, mode);
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
        // Difference-carrying action text from the newest cycle in the run.
        let action_summary = primary_action_text(&representative);

        let mut entry = representative;
        if let Value::Object(map) = &mut entry {
            map.insert("disposition".to_string(), json!(disp.as_str()));
            map.insert("repeat_count".to_string(), json!(repeat_count));
            map.insert("cycle_number_first".to_string(), json!(cycle_first));
            map.insert("cycle_number_last".to_string(), json!(cycle_last));
            match disp {
                Disposition::Deferring => {
                    map.insert("deferring_to".to_string(), json!(goals));
                    let collapsed = match mode {
                        CollapseMode::Strict => {
                            // Legacy second-half phrasing — do not change.
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
                            format!("Deferring to {goal_txt}{times}")
                        }
                        CollapseMode::Relaxed => {
                            // Difference-carrying first-half phrasing. The ×N
                            // repeat label is rendered separately from the count.
                            if goals.is_empty() {
                                "no-action: deferring to active engineer".to_string()
                            } else {
                                format!(
                                    "no-action: deferring to active engineer on {}",
                                    goals.join(", ")
                                )
                            }
                        }
                    };
                    map.insert("collapsed_summary".to_string(), json!(collapsed));
                }
                Disposition::Reasoning if repeat_count >= LOOP_REPEAT_THRESHOLD => {
                    map.insert("loop_suspected".to_string(), json!(true));
                }
                _ => {}
            }
            // Relaxed mode guarantees every row (progressing / reasoning that did
            // not already get a deferral summary) has a non-empty,
            // difference-carrying summary — never a blank or count-boilerplate
            // cell. Strict mode is left byte-identical to legacy.
            if mode == CollapseMode::Relaxed && !map.contains_key("collapsed_summary") {
                let s = action_summary.unwrap_or_else(|| default_relaxed_summary(disp).to_string());
                map.insert("collapsed_summary".to_string(), json!(s));
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

    // ========================================================================
    // Issue #21 — Cycle History (FIRST HALF) relaxed-collapse contract.
    //
    // These tests specify the NEW `CollapseMode` / `collapse_reports_with` API
    // and the difference-carrying relaxed summaries used by `/api/ooda-cycles`.
    // The existing `collapse_reports` (STRICT) behaviour — used by the PRESERVED
    // second half (`/api/ooda-thinking`) — must remain byte-identical.
    // ========================================================================

    /// A production-shaped boilerplate *reasoning* cycle whose summary carries
    /// the volatile "Cycle #N — P priorities considered, S of S actions
    /// succeeded · … · I open issues · working tree clean" text. The outcome
    /// `detail` is identical across cycles; only the summary numbers differ, so
    /// consecutive cycles differ **only** in cosmetic digits (the exact
    /// production symptom in deploy #21).
    fn boilerplate_reasoning_report(
        cycle: u64,
        priorities: u64,
        succeeded: u64,
        issues: u64,
    ) -> Value {
        json!({
            "cycle_number": cycle,
            "summary": format!(
                "Cycle #{cycle} — {priorities} priorities considered, {succeeded} of {succeeded} actions succeeded · 2 goals tracked · {issues} open issues · working tree clean"
            ),
            "outcomes": [{
                "action_kind": "Reflect",
                "success": true,
                "detail": "re-evaluated priorities; no action selected",
            }],
        })
    }

    #[test]
    fn collapse_reports_is_exact_alias_for_strict_mode() {
        // Regression guard for the PRESERVED second half: the public
        // `collapse_reports` must be byte-identical to
        // `collapse_reports_with(_, Strict)` across a mixed fixture.
        let fixture = vec![
            deferral_report(30, "g1"),
            deferral_report(29, "g1"),
            progress_report(28, "g1"),
            reasoning_report(27, "stuck on the same thing"),
            reasoning_report(26, "stuck on the same thing"),
            reasoning_report(25, "stuck on the same thing"),
        ];
        let via_alias = collapse_reports(fixture.clone());
        let via_strict = collapse_reports_with(fixture, CollapseMode::Strict);
        assert_eq!(
            via_alias, via_strict,
            "collapse_reports must remain an exact alias of Strict mode"
        );
    }

    #[test]
    fn relaxed_collapses_cosmetically_different_reasoning_cycles() {
        // Two boilerplate cycles differing ONLY in cycle number + volatile
        // counts must collapse to a single row under Relaxed.
        let reports = vec![
            boilerplate_reasoning_report(1040, 3, 2, 19),
            boilerplate_reasoning_report(1039, 3, 2, 20),
        ];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(
            out.len(),
            1,
            "cosmetic-only (digit) variance must collapse under Relaxed"
        );
        assert_eq!(out[0]["repeat_count"], 2);
        assert_eq!(out[0]["cycle_number_first"], 1040, "first = newest");
        assert_eq!(out[0]["cycle_number_last"], 1039, "last = oldest");
    }

    #[test]
    fn strict_keeps_cosmetically_different_reasoning_cycles_separate() {
        // The SAME input under Strict must NOT collapse — this is the bug the
        // relaxed mode fixes, and proves Strict (second half) is unchanged.
        let reports = vec![
            boilerplate_reasoning_report(1040, 3, 2, 19),
            boilerplate_reasoning_report(1039, 3, 2, 20),
        ];
        let out = collapse_reports_with(reports, CollapseMode::Strict);
        assert_eq!(
            out.len(),
            2,
            "Strict mode must keep digit-differing cycles separate (legacy behaviour)"
        );
    }

    #[test]
    fn relaxed_keeps_genuinely_different_decisions_separate() {
        // Digit masking must NOT merge cycles that decided different things.
        let reports = vec![
            json!({
                "cycle_number": 11,
                "summary": "Cycle #11 — decided to refactor the parser",
                "outcomes": [{"action_kind": "Reflect", "success": true, "detail": "chose parser refactor"}],
            }),
            json!({
                "cycle_number": 10,
                "summary": "Cycle #10 — decided to write documentation",
                "outcomes": [{"action_kind": "Reflect", "success": true, "detail": "chose docs work"}],
            }),
        ];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(
            out.len(),
            2,
            "different decision TEXT must not merge under Relaxed"
        );
    }

    #[test]
    fn relaxed_deferral_summary_is_difference_carrying() {
        let reports = vec![deferral_report(10, "g1"), deferral_report(9, "g1")];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0]["collapsed_summary"].as_str().unwrap(),
            "no-action: deferring to active engineer on g1",
            "relaxed deferral summary must use the difference-carrying phrasing"
        );
    }

    #[test]
    fn strict_deferral_summary_retains_legacy_phrasing() {
        // The second-half phrasing MUST NOT change to the relaxed phrasing.
        let reports = vec![deferral_report(10, "g1"), deferral_report(9, "g1")];
        let out = collapse_reports_with(reports, CollapseMode::Strict);
        assert_eq!(out.len(), 1);
        let summary = out[0]["collapsed_summary"].as_str().unwrap();
        assert!(
            summary.starts_with("Deferring to an active engineer on g1"),
            "strict deferral summary must retain the legacy phrasing, got: {summary}"
        );
    }

    #[test]
    fn relaxed_collapsed_summary_is_never_empty_for_any_disposition() {
        // The blank / count-boilerplate cell defect is gone: EVERY relaxed row
        // carries a non-empty collapsed_summary.
        let deferring =
            collapse_reports_with(vec![deferral_report(3, "g1")], CollapseMode::Relaxed);
        let progressing =
            collapse_reports_with(vec![progress_report(2, "g1")], CollapseMode::Relaxed);
        let reasoning = collapse_reports_with(
            vec![boilerplate_reasoning_report(1, 3, 2, 20)],
            CollapseMode::Relaxed,
        );
        for (name, out) in [
            ("deferring", deferring),
            ("progressing", progressing),
            ("reasoning", reasoning),
        ] {
            let s = out[0]["collapsed_summary"].as_str().unwrap_or("");
            assert!(
                !s.is_empty(),
                "{name} row must carry a non-empty collapsed_summary (no blank/boilerplate cell)"
            );
        }
    }

    #[test]
    fn relaxed_reasoning_summary_strips_count_boilerplate() {
        // A relaxed reasoning row must NOT surface the "N priorities
        // considered, M of M actions succeeded … working tree clean"
        // boilerplate verbatim as its summary.
        let out = collapse_reports_with(
            vec![boilerplate_reasoning_report(42, 3, 2, 20)],
            CollapseMode::Relaxed,
        );
        let s = out[0]["collapsed_summary"].as_str().unwrap_or("");
        assert!(!s.is_empty(), "reasoning summary must be non-empty");
        assert!(
            !s.contains("priorities considered") || !s.contains("working tree clean"),
            "reasoning summary must not be the raw count-boilerplate, got: {s}"
        );
    }

    #[test]
    fn relaxed_progress_cycles_are_never_collapsed() {
        let reports = vec![progress_report(10, "g1"), progress_report(9, "g1")];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(
            out.len(),
            2,
            "forward-progress cycles are never collapsed, even under Relaxed"
        );
    }

    #[test]
    fn relaxed_deferral_run_is_never_flagged_as_loop() {
        // A healthy deferral run collapses quietly with just its count — it is
        // NEVER dressed up as a stuck loop.
        let reports = vec![
            deferral_report(6, "g1"),
            deferral_report(5, "g1"),
            deferral_report(4, "g1"),
            deferral_report(3, "g1"),
        ];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["repeat_count"], 4);
        assert!(
            out[0].get("loop_suspected").is_none(),
            "a healthy deferral run must never be flagged as a loop"
        );
    }

    #[test]
    fn relaxed_reasoning_run_is_flagged_as_loop() {
        // A genuinely stuck reasoning loop (>= threshold) is still flagged
        // under Relaxed.
        let reports = vec![
            boilerplate_reasoning_report(5, 3, 2, 20),
            boilerplate_reasoning_report(4, 3, 2, 20),
            boilerplate_reasoning_report(3, 3, 2, 20),
        ];
        let out = collapse_reports_with(reports, CollapseMode::Relaxed);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["repeat_count"], 3);
        assert_eq!(out[0]["loop_suspected"], true);
    }
}
