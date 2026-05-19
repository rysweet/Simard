//! Parse-failure visibility for decide/orient brain invocations (issue #1890).
//!
//! Closes the silent-fallback gap that #1890 documented: before this module,
//! `decide_with_brain` and `orient_with_brain` swallowed every `Err(_)` from
//! the LLM brain into the deterministic fallback with no visible record. The
//! cycle still ran, `cycle_N.json` still showed a decision, and goals could
//! stall for days at 0.00% before anyone noticed.
//!
//! This module owns the **four visibility channels** that every brain
//! parse-failure now fires (see `docs/reference/ooda-brain-parse-failure-record.md`):
//!
//!   1. `tracing::error!` (target `simard::ooda_brain`) with structured fields.
//!   2. `record_metric("brain_parse_failure", 1.0, …)` to `~/.simard/metrics/metrics.jsonl`.
//!   3. A [`ParseFailureRecord`] returned to the call site, embedded in the
//!      `BrainJudgmentRecord.parse_failure` field that lands in
//!      `~/.simard/cycle_reports/cycle_*.json`.
//!   4. Throttled `gh issue create` at `>= ISSUE_ESCALATION_THRESHOLD`
//!      consecutive failures per `(phase, goal_id)`.
//!
//! The module is **stub-only** in this commit (Step 7 — TDD). The behaviour
//! is pinned by the failing tests in `src/ooda_loop/tests_parse_failure_1890.rs`
//! and `src/ooda_brain/parse_failure_tests.rs`. Step 8 will wire the four
//! channels through the two call sites.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::BrainPhase;

/// Cap fed to `truncate_to_char_boundary` for `raw_response_truncated`.
/// Matches the existing 8 KiB cap used by `truncate_for_log_pub` in
/// `rustyclawd.rs` so the same body limit applies to both the legacy log
/// path and the new on-disk record (resolution A8).
pub const RAW_RESPONSE_TRUNCATE_BYTES: usize = 8192;

/// Throttle threshold for the `gh issue create` channel. Mirrors the
/// `spawn_engineer` precedent from PR #1711 (resolution A6).
pub const ISSUE_ESCALATION_THRESHOLD: u32 = 3;

/// `--repo` slug for `gh issue create`. Compile-time constant so escalations
/// cannot be silently redirected by an env var; forks rebuilding this binary
/// must edit the constant. Matches the pattern in `stewardship/merge_authority.rs`.
pub const ESCALATION_REPO_SLUG: &str = "rysweet/Simard";

/// One brain-invocation failure record, embedded on `BrainJudgmentRecord`.
///
/// Every field is always populated — there are no `Option` fields. Operators
/// that need to distinguish "absent" from "empty string" can use `prompt_version`
/// (empty means the prompt-store served the embedded fallback).
///
/// See `docs/reference/ooda-brain-parse-failure-record.md` for the full
/// schema rationale.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParseFailureRecord {
    /// `"decide"` or `"orient"`. Lowercased to match `BrainPhase` serde.
    pub phase: String,
    /// Internal goal id (never a user-controlled string at the call site;
    /// origin via meeting decisions / goal-curation is documented as
    /// opaque-bytes for `Command::args`).
    pub goal_id: String,
    /// `err.to_string()` of the `SimardError` the brain returned — `Display`,
    /// never `Debug`, so future `SimardError` variants that grow private
    /// fields cannot leak.
    pub error_message: String,
    /// The complete model response, truncated via
    /// `truncate_to_char_boundary` at [`RAW_RESPONSE_TRUNCATE_BYTES`] on a
    /// UTF-8 boundary. Empty for non-parse `Err` variants where no body
    /// was returned.
    pub raw_response_truncated: String,
    /// Prompt asset name (`"ooda_decide.md"` / `"ooda_orient.md"`). Sourced
    /// from the existing `DECIDE_PROMPT_NAME` / `ORIENT_PROMPT_NAME`
    /// `&'static str` constants the call site already has.
    pub prompt_name: String,
    /// 12-char sha256 prefix of the prompt-asset content the brain loaded
    /// (`prompt_store::current_version`). Empty when the embedded fallback
    /// was served.
    pub prompt_version: String,
    /// Consecutive `(phase, goal_id)` parse failures up to and including
    /// this one. Resets to 0 on the next successful parse for the same key.
    pub consecutive_count: u32,
    /// Reserved for future retry-with-feedback. Always `false` in this
    /// release; the JSON shape is kept stable so retry can land without
    /// a schema bump.
    pub retry_attempted: bool,
    /// RFC 3339 UTC timestamp of the failure. Set by [`record_parse_failure`]
    /// so all four channels agree on the moment.
    pub timestamp: String,
}

/// Process-local `(phase, goal_id) -> consecutive_count` map. Reset to 0 by
/// [`reset_consecutive_count`] on the next successful parse for the same key.
/// Documented cross-restart loss is acceptable — worst case is one extra
/// `gh issue` per daemon restart loop, which is itself a signal worth investigating.
#[allow(dead_code)] // Step 8 wires this into the _with_brain call sites.
fn counters() -> &'static Mutex<HashMap<(BrainPhase, String), u32>> {
    static CTRS: OnceLock<Mutex<HashMap<(BrainPhase, String), u32>>> = OnceLock::new();
    CTRS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Increment the `(phase, goal_id)` counter and return the new value.
/// `pub(crate)` so the `_with_brain` call sites can call this directly.
#[allow(dead_code)] // Step 8 wires this into the _with_brain call sites.
pub(crate) fn bump_consecutive_count(phase: BrainPhase, goal_id: &str) -> u32 {
    let mut guard = counters()
        .lock()
        .expect("parse_failure counter mutex poisoned");
    let entry = guard.entry((phase, goal_id.to_string())).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

/// Reset the `(phase, goal_id)` counter to 0 (called on the next successful
/// parse for the same key).
#[allow(dead_code)] // Step 8 wires this into the _with_brain call sites.
pub(crate) fn reset_consecutive_count(phase: BrainPhase, goal_id: &str) {
    let mut guard = counters()
        .lock()
        .expect("parse_failure counter mutex poisoned");
    guard.remove(&(phase, goal_id.to_string()));
}

/// Test-only: peek at the current count without mutating it.
#[cfg(test)]
pub(crate) fn peek_consecutive_count(phase: BrainPhase, goal_id: &str) -> u32 {
    let guard = counters()
        .lock()
        .expect("parse_failure counter mutex poisoned");
    guard
        .get(&(phase, goal_id.to_string()))
        .copied()
        .unwrap_or(0)
}

/// Test-only: serialize tests that need a known starting state by holding a
/// shared mutex for the duration of the test. Returning a guard scopes the
/// lock to the caller. Required because the `(phase, goal_id) -> count` map
/// is process-global and `cargo test` runs tests in parallel.
#[cfg(test)]
pub(crate) fn test_serial_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("parse_failure test mutex poisoned")
}

/// Test-only: clear a specific `(phase, goal_id)` entry. Scoped clearance is
/// safer than a global wipe because a global wipe can race with concurrent
/// tests that touch unrelated keys.
#[cfg(test)]
pub(crate) fn reset_consecutive_count_for_tests(phase: BrainPhase, goal_id: &str) {
    reset_consecutive_count(phase, goal_id);
}

/// Build a [`ParseFailureRecord`] for a brain-invocation failure and fire the
/// four visibility channels (tracing, metric, return-for-embed, conditional
/// `gh issue create`).
///
/// STEP 7 STATUS: stub. This implementation only constructs the record and
/// bumps the counter — it does NOT yet fire tracing, the metric, or `gh`.
/// The TDD tests assert the wired behaviour; they fail until Step 8 lands
/// the real implementation.
#[allow(dead_code)] // Step 8 wires this into the _with_brain call sites.
pub(crate) fn record_parse_failure(
    phase: BrainPhase,
    goal_id: &str,
    err: &crate::error::SimardError,
    raw_response: &str,
    prompt_name: &'static str,
    prompt_version: String,
) -> ParseFailureRecord {
    let consecutive_count = bump_consecutive_count(phase, goal_id);

    let mut raw_truncated = raw_response.to_string();
    crate::util::string_truncate::truncate_to_char_boundary(
        &mut raw_truncated,
        RAW_RESPONSE_TRUNCATE_BYTES,
    );

    let record = ParseFailureRecord {
        phase: phase_to_string(phase),
        goal_id: goal_id.to_string(),
        error_message: err.to_string(),
        raw_response_truncated: raw_truncated,
        prompt_name: prompt_name.to_string(),
        prompt_version,
        consecutive_count,
        retry_attempted: false,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    // STEP 8 wires the four channels here. Stub on purpose so TDD tests
    // can drive the wire-up.

    record
}

#[allow(dead_code)] // Step 8 wires this into the _with_brain call sites.
fn phase_to_string(phase: BrainPhase) -> String {
    match phase {
        BrainPhase::Act => "act".to_string(),
        BrainPhase::Decide => "decide".to_string(),
        BrainPhase::Orient => "orient".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SimardError;

    fn sample_err() -> SimardError {
        SimardError::AdapterInvocationFailed {
            base_type: "ooda-decide-brain".to_string(),
            reason: "no JSON object; raw_response=\"OK\"".to_string(),
        }
    }

    #[test]
    fn record_round_trips_through_json() {
        let rec = ParseFailureRecord {
            phase: "decide".to_string(),
            goal_id: "g1".to_string(),
            error_message: "boom".to_string(),
            raw_response_truncated: "OK".to_string(),
            prompt_name: "ooda_decide.md".to_string(),
            prompt_version: "deadbeef".to_string(),
            consecutive_count: 2,
            retry_attempted: false,
            timestamp: "2026-05-19T04:44:26Z".to_string(),
        };
        let json = serde_json::to_string(&rec).unwrap();
        // Every field MUST round-trip — no Option fields, no skip_serializing.
        assert!(json.contains("\"phase\":\"decide\""), "got: {json}");
        assert!(json.contains("\"goal_id\":\"g1\""), "got: {json}");
        assert!(json.contains("\"error_message\":\"boom\""), "got: {json}");
        assert!(
            json.contains("\"raw_response_truncated\":\"OK\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"prompt_name\":\"ooda_decide.md\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"prompt_version\":\"deadbeef\""),
            "got: {json}"
        );
        assert!(json.contains("\"consecutive_count\":2"), "got: {json}");
        assert!(json.contains("\"retry_attempted\":false"), "got: {json}");
        assert!(
            json.contains("\"timestamp\":\"2026-05-19T04:44:26Z\""),
            "got: {json}"
        );

        let back: ParseFailureRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn record_parse_failure_populates_all_fields() {
        let err = sample_err();
        let rec = record_parse_failure(
            BrainPhase::Decide,
            "improve-dashboard",
            &err,
            "OK",
            "ooda_decide.md",
            "abc123def456".to_string(),
        );
        assert_eq!(rec.phase, "decide");
        assert_eq!(rec.goal_id, "improve-dashboard");
        assert!(rec.error_message.contains("ooda-decide-brain"));
        assert!(rec.error_message.contains("no JSON object"));
        assert_eq!(rec.raw_response_truncated, "OK");
        assert_eq!(rec.prompt_name, "ooda_decide.md");
        assert_eq!(rec.prompt_version, "abc123def456");
        assert_eq!(rec.consecutive_count, 1);
        assert!(!rec.retry_attempted);
        assert!(!rec.timestamp.is_empty(), "timestamp must be populated");
    }

    #[test]
    fn record_parse_failure_truncates_raw_response_at_8_kib() {
        let huge = "a".repeat(20_000);
        let err = sample_err();
        let rec = record_parse_failure(
            BrainPhase::Orient,
            "g1",
            &err,
            &huge,
            "ooda_orient.md",
            String::new(),
        );
        assert!(
            rec.raw_response_truncated.len() <= RAW_RESPONSE_TRUNCATE_BYTES,
            "raw_response_truncated len {} exceeds {} cap",
            rec.raw_response_truncated.len(),
            RAW_RESPONSE_TRUNCATE_BYTES,
        );
    }

    #[test]
    fn record_parse_failure_truncates_on_utf8_boundary() {
        // Anti-regression: char-boundary safety — a String full of 3-byte
        // characters must not be truncated mid-codepoint (would panic in
        // s.truncate() on bad boundary).

        let multibyte: String = std::iter::repeat('字').take(5000).collect();
        let err = sample_err();
        let rec = record_parse_failure(
            BrainPhase::Decide,
            "g1",
            &err,
            &multibyte,
            "ooda_decide.md",
            String::new(),
        );
        // Round-trip through serde — would fail if truncation produced
        // invalid UTF-8.
        let json = serde_json::to_string(&rec).expect("must serialize after truncation");
        let _back: ParseFailureRecord = serde_json::from_str(&json).expect("must round-trip");
        assert!(rec.raw_response_truncated.len() <= RAW_RESPONSE_TRUNCATE_BYTES);
    }

    #[test]
    fn consecutive_count_increments_per_phase_goal_id_pair() {
        let err = sample_err();
        // Use a goal_id unique to this test to avoid racing with other
        // tests in the same module that share the global counter map.
        let goal = "consecutive_count_increments_per_phase_goal_id_pair-goal";
        let r1 = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        let r2 = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        let r3 = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        assert_eq!(r1.consecutive_count, 1);
        assert_eq!(r2.consecutive_count, 2);
        assert_eq!(r3.consecutive_count, 3);
    }

    #[test]
    fn consecutive_count_tracked_separately_per_phase() {
        let err = sample_err();
        let goal = "consecutive_count_tracked_separately_per_phase-goal";
        let d = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        let o = record_parse_failure(BrainPhase::Orient, goal, &err, "x", "p", String::new());
        assert_eq!(d.consecutive_count, 1);
        assert_eq!(
            o.consecutive_count, 1,
            "orient counter must not collide with decide"
        );
    }

    #[test]
    fn consecutive_count_tracked_separately_per_goal() {
        let err = sample_err();
        let a = record_parse_failure(
            BrainPhase::Decide,
            "consec-per-goal-A",
            &err,
            "x",
            "p",
            String::new(),
        );
        let b = record_parse_failure(
            BrainPhase::Decide,
            "consec-per-goal-B",
            &err,
            "x",
            "p",
            String::new(),
        );
        assert_eq!(a.consecutive_count, 1);
        assert_eq!(
            b.consecutive_count, 1,
            "goal-b counter must not collide with goal-a"
        );
    }

    #[test]
    fn reset_consecutive_count_clears_counter_for_pair() {
        let err = sample_err();
        let goal = "reset_consecutive_count_clears_counter_for_pair-goal";
        let _ = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        let _ = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        assert_eq!(peek_consecutive_count(BrainPhase::Decide, goal), 2);
        reset_consecutive_count(BrainPhase::Decide, goal);
        assert_eq!(peek_consecutive_count(BrainPhase::Decide, goal), 0);
        // Next failure starts fresh at 1.
        let next = record_parse_failure(BrainPhase::Decide, goal, &err, "x", "p", String::new());
        assert_eq!(next.consecutive_count, 1);
    }

    #[test]
    fn reset_consecutive_count_for_one_pair_does_not_affect_others() {
        let err = sample_err();
        let _ = record_parse_failure(
            BrainPhase::Decide,
            "reset-isolation-A",
            &err,
            "x",
            "p",
            String::new(),
        );
        let _ = record_parse_failure(
            BrainPhase::Decide,
            "reset-isolation-B",
            &err,
            "x",
            "p",
            String::new(),
        );
        reset_consecutive_count(BrainPhase::Decide, "reset-isolation-A");
        assert_eq!(
            peek_consecutive_count(BrainPhase::Decide, "reset-isolation-A"),
            0
        );
        assert_eq!(
            peek_consecutive_count(BrainPhase::Decide, "reset-isolation-B"),
            1
        );
    }

    #[test]
    fn record_parse_failure_uses_display_not_debug_for_error_message() {
        // Defense: if the implementation ever switches to {:?}, this test
        // catches it — Debug for AdapterInvocationFailed prints field names
        // (base_type:, reason:) while Display reads as a sentence.

        let err = SimardError::AdapterInvocationFailed {
            base_type: "ooda-decide-brain".to_string(),
            reason: "decide brain returned an empty response".to_string(),
        };
        let rec = record_parse_failure(
            BrainPhase::Decide,
            "g1",
            &err,
            "",
            "ooda_decide.md",
            String::new(),
        );
        // Display format from src/error/display.rs:
        //   "base type 'ooda-decide-brain' failed during invocation: ..."
        assert!(
            rec.error_message.contains("failed during invocation"),
            "expected Display format, got: {}",
            rec.error_message,
        );
        assert!(
            !rec.error_message.contains("AdapterInvocationFailed"),
            "Debug format leaked variant name into error_message: {}",
            rec.error_message,
        );
    }

    #[test]
    fn record_serializes_safely_with_quotes_and_braces_in_raw_response() {
        // JSON-injection resistance: the typed struct + serde_json must
        // escape `"}` correctly so a hostile model response can't break the
        // surrounding JSON.

        let hostile = r#"OK"} ,"injected":"bad"#;
        let err = sample_err();
        let rec = record_parse_failure(
            BrainPhase::Decide,
            "g1",
            &err,
            hostile,
            "ooda_decide.md",
            String::new(),
        );
        let json = serde_json::to_string(&rec).unwrap();
        let back: ParseFailureRecord = serde_json::from_str(&json).expect("must round-trip");
        assert_eq!(back.raw_response_truncated, hostile);
    }
}
