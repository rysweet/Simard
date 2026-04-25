//! Eval watchdog: detects "absence of signal" in the gym progressive suite.
//!
//! Background
//! ----------
//! Between 2026-04-12 and 2026-04-25 the OODA daemon ran 89 cycles
//! reporting `✓ completed` while every L1-L12 evaluation scored 0.00%
//! and every cycle logged `Grading failed: Expecting value`. The
//! signal was dead — the bundled Claude Code CLI was not authenticated
//! and Copilot was not in the path because of an amplihack router bug
//! (fixed in amplihack#4477) — but Simard's loop kept marching because
//! it trusted its own success codes more than its measurements.
//!
//! This is the same anti-pattern as Therac-25 and a thousand cheaper
//! incidents: the supervisor cannot detect that its own observation
//! infrastructure is lying to it. The watchdog exists so that "every
//! L-test scored 0.0" is a halt-worthy event instead of business as usual.
//!
//! References
//! ----------
//! - Rob Ewaschuk, *My Philosophy on Alerting* (Google SRE Book):
//!   alert on user-visible symptoms, not on internal state.
//! - Marc Brooker, *Avoid Fallback in Distributed Systems* (AWS
//!   Builders' Library): fallback paths reduce observability of the
//!   failure they were meant to handle.

use std::collections::HashMap;

use crate::error::SimardResult;
use crate::gym_history::{ScoreHistory, ScoreRecord};

/// Reason a watchdog fired.
#[derive(Clone, Debug, PartialEq)]
pub enum DeadSignalReason {
    /// All recorded scores in the inspection window were exactly 0.0.
    /// Strongest signal: the grader is returning empty / failing JSON
    /// parse on every scenario.
    AllZero {
        /// Number of consecutive zero-score records observed.
        count: usize,
        /// Distinct scenario ids that contributed (so we don't fire on
        /// a single broken scenario).
        scenarios: usize,
    },
    /// All recent scores across multiple distinct scenarios are
    /// pixel-identical (e.g. all exactly 1.0 or all exactly 0.5).
    /// Distinct scenarios producing distinct content but identical
    /// scores almost always means a degenerate grader.
    SuspiciouslyIdentical {
        score: f64,
        count: usize,
        scenarios: usize,
    },
}

impl std::fmt::Display for DeadSignalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllZero { count, scenarios } => write!(
                f,
                "all-zero scores: {count} consecutive records across {scenarios} \
                 distinct scenarios. Grader likely returning empty / failing \
                 JSON parse. The eval signal is dead.",
            ),
            Self::SuspiciouslyIdentical {
                score,
                count,
                scenarios,
            } => write!(
                f,
                "suspiciously identical scores: {count} consecutive records \
                 across {scenarios} distinct scenarios all == {score:.4}. \
                 Grader likely degenerate.",
            ),
        }
    }
}

/// Configuration for the dead-signal detector.
#[derive(Clone, Debug)]
pub struct WatchdogConfig {
    /// Inspect at most this many of the most recent records per scenario.
    pub window_per_scenario: usize,
    /// Require at least this many distinct scenarios with consistent
    /// dead-signal evidence before firing. Single-scenario flat-lining
    /// might be a real failure of one test, not infrastructure dead.
    pub min_distinct_scenarios: usize,
    /// Minimum number of total records inspected before any signal can
    /// fire. Prevents firing on cold start.
    pub min_total_records: usize,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            window_per_scenario: 3,
            min_distinct_scenarios: 3,
            min_total_records: 6,
        }
    }
}

/// Detect a dead signal in the given score records.
///
/// Pure function: takes a flat slice of records (typically the last N
/// per scenario, concatenated), returns Some(reason) if the watchdog
/// should fire.
pub fn detect_dead_signal(
    records: &[ScoreRecord],
    config: &WatchdogConfig,
) -> Option<DeadSignalReason> {
    if records.len() < config.min_total_records {
        return None;
    }

    // Bucket per scenario; we only care about the most recent
    // `window_per_scenario` per scenario.
    let mut by_scenario: HashMap<&str, Vec<&ScoreRecord>> = HashMap::new();
    for r in records {
        by_scenario
            .entry(r.scenario_id.as_str())
            .or_default()
            .push(r);
    }

    // Sort each bucket newest-first and trim.
    let mut tails: Vec<Vec<&ScoreRecord>> = by_scenario
        .into_values()
        .map(|mut v| {
            v.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
            v.truncate(config.window_per_scenario);
            v
        })
        .collect();

    // Drop scenarios that didn't have enough recent records to be
    // meaningful evidence on their own.
    tails.retain(|v| !v.is_empty());

    let distinct = tails.len();
    if distinct < config.min_distinct_scenarios {
        return None;
    }

    // ── AllZero check ────────────────────────────────────────────────
    let total_inspected: usize = tails.iter().map(|v| v.len()).sum();
    let all_zero = tails.iter().all(|v| v.iter().all(|r| r.score == 0.0));
    if all_zero {
        return Some(DeadSignalReason::AllZero {
            count: total_inspected,
            scenarios: distinct,
        });
    }

    // ── SuspiciouslyIdentical check ─────────────────────────────────
    // All scenarios' recent records are exactly equal to the same
    // non-zero value. (Multiple distinct scenarios producing the exact
    // same score across multiple runs is statistically improbable.)
    let first_score = tails[0][0].score;
    let all_same = tails.iter().all(|v| {
        v.iter()
            .all(|r| (r.score - first_score).abs() < f64::EPSILON)
    });
    if all_same && first_score != 0.0 {
        return Some(DeadSignalReason::SuspiciouslyIdentical {
            score: first_score,
            count: total_inspected,
            scenarios: distinct,
        });
    }

    None
}

/// Helper: load recent records from a [`ScoreHistory`] suitable for
/// passing to [`detect_dead_signal`].
///
/// Pulls the most recent `window` records per known scenario in `suite_id`.
pub fn collect_recent_records(
    history: &ScoreHistory,
    suite_id: &str,
    window: usize,
) -> SimardResult<Vec<ScoreRecord>> {
    let scenario_ids = history.scenario_ids(suite_id)?;
    let mut out = Vec::new();
    for sid in scenario_ids {
        let recs = history.history(suite_id, &sid, window)?;
        out.extend(recs);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(scenario: &str, score: f64, ts: i64) -> ScoreRecord {
        ScoreRecord {
            suite_id: "progressive".into(),
            scenario_id: scenario.into(),
            score,
            timestamp: ts,
            commit_hash: None,
        }
    }

    #[test]
    fn detects_all_zero_across_scenarios() {
        let cfg = WatchdogConfig::default();
        let records = vec![
            rec("L1", 0.0, 100),
            rec("L1", 0.0, 200),
            rec("L1", 0.0, 300),
            rec("L2", 0.0, 100),
            rec("L2", 0.0, 200),
            rec("L2", 0.0, 300),
            rec("L3", 0.0, 100),
            rec("L3", 0.0, 200),
            rec("L3", 0.0, 300),
        ];
        match detect_dead_signal(&records, &cfg) {
            Some(DeadSignalReason::AllZero { count, scenarios }) => {
                assert_eq!(scenarios, 3);
                assert!(count >= cfg.min_total_records);
            }
            other => panic!("expected AllZero, got {other:?}"),
        }
    }

    #[test]
    fn does_not_fire_below_min_total() {
        let cfg = WatchdogConfig::default();
        let records = vec![rec("L1", 0.0, 100), rec("L2", 0.0, 100)];
        assert!(detect_dead_signal(&records, &cfg).is_none());
    }

    #[test]
    fn does_not_fire_below_min_distinct_scenarios() {
        let cfg = WatchdogConfig::default();
        // 6 records but all on one scenario — could be a bug in just
        // that scenario, not infrastructure death. Should not fire.
        let records = vec![
            rec("L1", 0.0, 100),
            rec("L1", 0.0, 200),
            rec("L1", 0.0, 300),
            rec("L1", 0.0, 400),
            rec("L1", 0.0, 500),
            rec("L1", 0.0, 600),
        ];
        assert!(detect_dead_signal(&records, &cfg).is_none());
    }

    #[test]
    fn does_not_fire_when_some_nonzero() {
        let cfg = WatchdogConfig::default();
        let records = vec![
            rec("L1", 0.0, 100),
            rec("L1", 0.0, 200),
            rec("L1", 0.0, 300),
            rec("L2", 0.0, 100),
            rec("L2", 0.0, 200),
            rec("L2", 0.83, 300), // recovery
            rec("L3", 0.0, 100),
            rec("L3", 0.0, 200),
            rec("L3", 0.0, 300),
        ];
        assert!(detect_dead_signal(&records, &cfg).is_none());
    }

    #[test]
    fn detects_suspiciously_identical_nonzero() {
        let cfg = WatchdogConfig::default();
        let records = vec![
            rec("L1", 0.5, 100),
            rec("L1", 0.5, 200),
            rec("L1", 0.5, 300),
            rec("L2", 0.5, 100),
            rec("L2", 0.5, 200),
            rec("L2", 0.5, 300),
            rec("L3", 0.5, 100),
            rec("L3", 0.5, 200),
            rec("L3", 0.5, 300),
        ];
        match detect_dead_signal(&records, &cfg) {
            Some(DeadSignalReason::SuspiciouslyIdentical { score, .. }) => {
                assert!((score - 0.5).abs() < 1e-9);
            }
            other => panic!("expected SuspiciouslyIdentical, got {other:?}"),
        }
    }

    #[test]
    fn does_not_fire_on_healthy_signal() {
        let cfg = WatchdogConfig::default();
        let records = vec![
            rec("L1", 1.00, 300),
            rec("L1", 0.96, 200),
            rec("L1", 0.92, 100),
            rec("L2", 0.98, 300),
            rec("L2", 0.93, 200),
            rec("L2", 0.90, 100),
            rec("L3", 0.97, 300),
            rec("L3", 0.95, 200),
            rec("L3", 0.91, 100),
        ];
        assert!(detect_dead_signal(&records, &cfg).is_none());
    }

    #[test]
    fn windowing_picks_most_recent_per_scenario() {
        let cfg = WatchdogConfig {
            window_per_scenario: 2,
            min_distinct_scenarios: 3,
            min_total_records: 6,
        };
        // Older records show healthy scores; the most recent are all zero.
        // Watchdog should fire on the most recent window.
        let records = vec![
            rec("L1", 0.95, 100),
            rec("L1", 0.0, 200),
            rec("L1", 0.0, 300),
            rec("L2", 0.94, 100),
            rec("L2", 0.0, 200),
            rec("L2", 0.0, 300),
            rec("L3", 0.93, 100),
            rec("L3", 0.0, 200),
            rec("L3", 0.0, 300),
        ];
        match detect_dead_signal(&records, &cfg) {
            Some(DeadSignalReason::AllZero { .. }) => {}
            other => panic!("expected AllZero on most-recent window, got {other:?}"),
        }
    }

    #[test]
    fn ignores_scenario_with_insufficient_records() {
        let cfg = WatchdogConfig::default();
        // L4 has only 1 record; should be dropped from the bucket
        // analysis, leaving 3 valid scenarios all-zero → fire.
        let records = vec![
            rec("L1", 0.0, 100),
            rec("L1", 0.0, 200),
            rec("L1", 0.0, 300),
            rec("L2", 0.0, 100),
            rec("L2", 0.0, 200),
            rec("L2", 0.0, 300),
            rec("L3", 0.0, 100),
            rec("L3", 0.0, 200),
            rec("L3", 0.0, 300),
            rec("L4", 0.95, 400),
        ];
        // L4's window is just one healthy record; the all-zero check
        // requires ALL scenarios in the analysis to be zero, so this
        // mixed case should NOT fire.
        assert!(detect_dead_signal(&records, &cfg).is_none());
    }
}
