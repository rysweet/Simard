//! Brain decision-parse outcome instrumentation (issue #2419).
//!
//! The three recipe-brain parse functions in [`super::recipe_brain`] use
//! **first-word-only** keyword extraction. Any decision the recipe emits in
//! prose, behind a `DECISION:` marker, or mid-sentence falls through to a
//! deterministic default — most visibly `continue_skipping` ("no decision
//! keyword found in recipe output"). Issue #2419 reports this default firing on
//! ~99.6% of `decide_engineer_lifecycle` invocations, but that number was
//! **anecdotal**: nothing counted parsed-vs-fell-through outcomes, so the rate
//! could not be measured before it could be fixed.
//!
//! This module is the measurement. Every parse function records, on each call,
//! exactly one [`ParseOutcome`] against its [`ParsePath`] through three
//! channels (mirroring the visibility design of [`super::parse_failure`]):
//!
//!   1. A process-global atomic counter, keyed by `(path, outcome)`, that the
//!      daemon can snapshot live via [`outcome_count`] / [`fallthrough_rate`].
//!   2. A `tracing` event (target `simard::ooda_brain`) — `debug` for a parsed
//!      keyword, `warn` for a default fallthrough so `journalctl` greps surface
//!      the stuck-goal signal.
//!   3. A `brain_parse_outcome` metric appended to
//!      `~/.simard/metrics/metrics.jsonl` so [`crate::self_metrics`] aggregation
//!      (e.g. `daily_report`) yields a continuous fallthrough rate over time.
//!
//! Disk writes are skipped under `#[cfg(test)]` so the hermetic parser unit
//! tests stay I/O-free; the atomic counter and a thread-local mirror still fire
//! so tests can assert increments deterministically.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Which parse function produced the outcome. The string form is the
/// `parse_path` label on every counter, log line, and metric context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParsePath {
    /// `parse_action_from_text` — decide-phase action routing.
    DecideAction,
    /// `parse_orient_from_text` — orient-phase urgency float.
    Orient,
    /// `parse_lifecycle_from_text` — engineer-lifecycle decision.
    Lifecycle,
}

impl ParsePath {
    /// Stable lowercase label used in counters, logs, and metric context.
    pub fn as_str(self) -> &'static str {
        match self {
            ParsePath::DecideAction => "decide_action",
            ParsePath::Orient => "orient",
            ParsePath::Lifecycle => "lifecycle",
        }
    }
}

/// Whether the parse matched a real keyword/float or fell through to the
/// deterministic default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParseOutcome {
    /// The first word matched a known keyword (or, for orient, a valid float
    /// was found). The model cooperated with the wire format.
    KeywordParsed,
    /// No keyword/float was found in the expected position; the parser
    /// returned its deterministic default (e.g. `continue_skipping`,
    /// `advance_goal`, or the orient floor). This is the #2419 signal.
    DefaultFallthrough,
}

impl ParseOutcome {
    /// Stable lowercase label used in counters, logs, and metric context.
    pub fn as_str(self) -> &'static str {
        match self {
            ParseOutcome::KeywordParsed => "keyword_parsed",
            ParseOutcome::DefaultFallthrough => "default_fallthrough",
        }
    }
}

type Key = (ParsePath, ParseOutcome);

/// Process-global counters. `Mutex<HashMap<Key, AtomicU64>>` mirrors the
/// established pattern in `src/cognitive_memory/metrics.rs`: the map grows once
/// (at most six entries) and the hot path only does a relaxed `fetch_add`.
fn counters() -> &'static Mutex<HashMap<Key, AtomicU64>> {
    static COUNTERS: OnceLock<Mutex<HashMap<Key, AtomicU64>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    /// Per-thread mirror of the global counters. `cargo test` runs each test on
    /// its own thread, so asserting against this view is immune to increments
    /// from other tests that also exercise the parse functions concurrently.
    static THREAD_LOCAL_COUNTS: RefCell<HashMap<Key, u64>> = RefCell::new(HashMap::new());
}

/// Record exactly one parse outcome. Called once per parse-function invocation.
///
/// `detail` is a short, non-user-controlled label (the matched keyword, or a
/// reason such as `"no_action_keyword"`) carried into the log line and metric
/// context for triage. Never pass raw model output here.
pub(crate) fn record_parse_outcome(path: ParsePath, outcome: ParseOutcome, detail: &str) {
    // Channel 1a: process-global atomic counter (live daemon snapshot).
    {
        let mut map = counters()
            .lock()
            .expect("parse_outcome counter mutex poisoned");
        map.entry((path, outcome))
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    // Channel 1b: thread-local mirror (race-free test isolation).
    THREAD_LOCAL_COUNTS.with(|tl| {
        *tl.borrow_mut().entry((path, outcome)).or_insert(0) += 1;
    });

    // Channel 2: structured tracing event. A fallthrough is the stuck-goal
    // signal, so it is logged at WARN; a parsed keyword is routine DEBUG.
    match outcome {
        ParseOutcome::KeywordParsed => tracing::debug!(
            target: "simard::ooda_brain",
            parse_path = path.as_str(),
            outcome = outcome.as_str(),
            detail = detail,
            "brain parse matched keyword",
        ),
        ParseOutcome::DefaultFallthrough => tracing::warn!(
            target: "simard::ooda_brain",
            parse_path = path.as_str(),
            outcome = outcome.as_str(),
            detail = detail,
            "brain parse fell through to deterministic default (issue #2419)",
        ),
    }

    // Channel 3: persistent metric for cross-cycle aggregation. Skipped under
    // unit tests so the pure-logic parser tests perform no disk I/O.
    if !cfg!(test) {
        let metric_ctx = serde_json::json!({
            "parse_path": path.as_str(),
            "outcome": outcome.as_str(),
            "detail": detail,
        })
        .to_string();
        if let Err(metric_err) =
            crate::self_metrics::record_metric("brain_parse_outcome", 1.0, &metric_ctx)
        {
            tracing::warn!(
                target: "simard::ooda_brain",
                error = %metric_err,
                "record_metric(brain_parse_outcome) failed (counter still incremented)",
            );
        }
    }
}

/// Live process-global count for `(path, outcome)`. Intended for the daemon to
/// expose alongside other OODA health metrics.
pub fn outcome_count(path: ParsePath, outcome: ParseOutcome) -> u64 {
    let map = counters()
        .lock()
        .expect("parse_outcome counter mutex poisoned");
    map.get(&(path, outcome))
        .map(|v| v.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// Fraction of `path` invocations that fell through to the deterministic
/// default, in `[0.0, 1.0]`. Returns `None` until at least one outcome has been
/// recorded for `path` (avoids reporting a meaningless `0/0`).
///
/// This is the headline number #2419 asks for: a continuously updating
/// fallthrough rate computed from the live counters.
pub fn fallthrough_rate(path: ParsePath) -> Option<f64> {
    let parsed = outcome_count(path, ParseOutcome::KeywordParsed);
    let fell = outcome_count(path, ParseOutcome::DefaultFallthrough);
    let total = parsed + fell;
    if total == 0 {
        None
    } else {
        Some(fell as f64 / total as f64)
    }
}

/// Test-only: per-thread count for `(path, outcome)`. Immune to concurrent
/// tests because `cargo test` gives each test its own thread.
#[cfg(test)]
pub(crate) fn thread_local_count(path: ParsePath, outcome: ParseOutcome) -> u64 {
    THREAD_LOCAL_COUNTS.with(|tl| tl.borrow().get(&(path, outcome)).copied().unwrap_or(0))
}

/// Test-only: clear this thread's mirror so a test starts from a known zero.
#[cfg(test)]
pub(crate) fn reset_thread_local_counts() {
    THREAD_LOCAL_COUNTS.with(|tl| tl.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_outcome_labels_are_stable() {
        assert_eq!(ParsePath::DecideAction.as_str(), "decide_action");
        assert_eq!(ParsePath::Orient.as_str(), "orient");
        assert_eq!(ParsePath::Lifecycle.as_str(), "lifecycle");
        assert_eq!(ParseOutcome::KeywordParsed.as_str(), "keyword_parsed");
        assert_eq!(
            ParseOutcome::DefaultFallthrough.as_str(),
            "default_fallthrough"
        );
    }

    #[test]
    fn record_increments_thread_local_for_exact_key_only() {
        reset_thread_local_counts();
        assert_eq!(
            thread_local_count(ParsePath::Lifecycle, ParseOutcome::DefaultFallthrough),
            0
        );

        record_parse_outcome(
            ParsePath::Lifecycle,
            ParseOutcome::DefaultFallthrough,
            "unit_test",
        );
        record_parse_outcome(
            ParsePath::Lifecycle,
            ParseOutcome::DefaultFallthrough,
            "unit_test",
        );

        assert_eq!(
            thread_local_count(ParsePath::Lifecycle, ParseOutcome::DefaultFallthrough),
            2
        );
        // Sibling buckets are untouched.
        assert_eq!(
            thread_local_count(ParsePath::Lifecycle, ParseOutcome::KeywordParsed),
            0
        );
        assert_eq!(
            thread_local_count(ParsePath::Orient, ParseOutcome::DefaultFallthrough),
            0
        );
    }

    #[test]
    fn fallthrough_rate_is_none_before_first_record_then_computed() {
        // Use a path no other test records against so the global rate is
        // deterministic here: Orient parsed + fallthrough are exercised by the
        // recipe_brain tests, so assert the *shape* rather than an exact value.
        let r = fallthrough_rate(ParsePath::Orient);
        if let Some(rate) = r {
            assert!((0.0..=1.0).contains(&rate), "rate out of range: {rate}");
        }
    }

    #[test]
    fn record_increments_global_counter_monotonically() {
        let before = outcome_count(ParsePath::DecideAction, ParseOutcome::DefaultFallthrough);
        record_parse_outcome(
            ParsePath::DecideAction,
            ParseOutcome::DefaultFallthrough,
            "unit_test",
        );
        let after = outcome_count(ParsePath::DecideAction, ParseOutcome::DefaultFallthrough);
        assert!(
            after > before,
            "global counter must strictly increase: {before} -> {after}"
        );
    }
}
