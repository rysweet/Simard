//! Bounded-execution primitive and `PartialReason` enum for the meeting
//! close pipeline.
//!
//! `with_timeout` runs `work` on a detached OS thread and waits up to
//! `budget` for it to finish. If the budget expires, the helper returns
//! [`Timeout`] **immediately** and the worker continues to drain in the
//! background; the worker is not killed because cancelling an arbitrary
//! synchronous closure mid-flight (e.g. one that holds a subprocess
//! handle) would corrupt later state. This is the deliberate
//! "abandon-not-kill" trade-off documented in
//! `docs/reference/meeting-close-lifecycle.md` (R-1).
//!
//! Why not `std::thread::scope`? A scope's destructor joins every
//! spawned thread, so a `recv_timeout` inside the scope cannot actually
//! return early — the scope blocks waiting for the join. The original
//! `MeetingBackend::generate_summary` used `thread::scope` and that is
//! the root cause of #1908 (the documented 90s timeout was never
//! enforced because the scope kept the parent blocked).
//!
//! This module is also the seam for a future Tokio migration: the
//! signature
//! `with_timeout<T>(Duration, impl FnOnce() -> T) -> Result<T, Timeout>`
//! is intentionally callable from sync code today and trivially wrappable
//! around `tokio::time::timeout(spawn_blocking(...))` later.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Sentinel returned by [`with_timeout`] when `work` did not complete
/// within `budget`. Carries no payload; callers infer the elapsed time
/// from the budget they passed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeout;

/// Run `work` on a detached worker thread and wait up to `budget` for
/// it to return.
///
/// `Ok(value)` — the worker returned within the budget; `value` is its
/// return value.
///
/// `Err(Timeout)` — the budget expired; the worker is **not** cancelled
/// and continues to drain in the background. It will be reaped on
/// process exit if it has not finished by then.
///
/// `work` must be `Send + 'static` because it is moved into a detached
/// thread. `T` must be `Send + 'static` because it crosses the channel.
pub fn with_timeout<T, F>(budget: Duration, work: F) -> Result<T, Timeout>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let r = work();
        // Receiver may have already given up on us; the send error is
        // expected in that case and is benign because the worker's
        // result is no longer needed.
        let _ = tx.send(r);
    });
    match rx.recv_timeout(budget) {
        Ok(v) => Ok(v),
        Err(_) => Err(Timeout),
    }
}

/// Reason a meeting close produced a **partial** handoff. Values are
/// serialized to the wire as snake_case strings via [`Display`]; this
/// is the value operators see in `WARN handoff_partial=true reason=...`
/// tracing lines, the REPL exit banner, and the
/// [`crate::meeting_backend::MeetingSummary::partial_reason`] field.
///
/// The enum is a closed set so log scrapers can rely on the wire
/// values; new variants require a docs update.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialReason {
    /// The master `MeetingBackend::close()` budget (default 60s,
    /// configurable via `SIMARD_MEETING_CLOSE_TIMEOUT_SECS`) expired
    /// before all phases completed. Remaining LLM/bridge phases are
    /// skipped and the close proceeds straight to persistence with the
    /// best-known partial data.
    CloseTimeout,

    /// The inner `agent.close()` budget (default 15s, configurable via
    /// `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS`) expired. The agent
    /// worker thread is abandoned (see module-level "abandon-not-kill"
    /// trade-off) and the close proceeds.
    AgentCloseTimeout,

    /// The inner LLM summary call exceeded its budget. The handoff
    /// summary string falls back to the
    /// "(partial — close timed out…)" sentinel and the close proceeds
    /// with metadata-only summary data.
    SummaryTimeout,

    /// The summarizer returned within budget but produced no
    /// extractable content (empty summary, no decisions, no actions,
    /// no questions). Treated as partial so operators are flagged to
    /// review the transcript by hand.
    SummaryEmpty,

    /// The cognitive-memory bridge `store_enriched_*` call exceeded
    /// its inner budget. Currently a no-op in production (no caller
    /// wires a bridge into `MeetingBackend::new_session`) but reserved
    /// for the future bridge-aware close pipeline.
    BridgeTimeout,

    /// A non-recoverable I/O error occurred while persisting the
    /// handoff bundle (after the atomic tmp-rename retry). The bundle
    /// directory may be incomplete; operators should check the bundle
    /// path printed by the REPL.
    PersistenceError,
}

impl PartialReason {
    /// Stable, machine-parseable wire string. Matches the
    /// `#[serde(rename_all = "snake_case")]` tag and the table in
    /// `docs/reference/meeting-close-lifecycle.md`.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            PartialReason::CloseTimeout => "close_timeout",
            PartialReason::AgentCloseTimeout => "agent_close_timeout",
            PartialReason::SummaryTimeout => "summary_timeout",
            PartialReason::SummaryEmpty => "summary_empty",
            PartialReason::BridgeTimeout => "bridge_timeout",
            PartialReason::PersistenceError => "persistence_error",
        }
    }
}

impl std::fmt::Display for PartialReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn with_timeout_returns_value_on_fast_work() {
        let result = with_timeout(Duration::from_secs(5), || 42_u32);
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn with_timeout_returns_timeout_when_work_exceeds_budget() {
        let start = std::time::Instant::now();
        let result = with_timeout(Duration::from_millis(50), || {
            thread::sleep(Duration::from_millis(500));
            "should not surface"
        });
        let elapsed = start.elapsed();
        assert_eq!(result, Err(Timeout));
        // Verify the parent returned within budget + a generous slack
        // window. Without `thread::spawn` (non-scoped) we would block
        // for the full 500ms — this assertion is the regression guard
        // for #1908.
        assert!(
            elapsed < Duration::from_millis(300),
            "with_timeout blocked parent for {elapsed:?}; expected <300ms"
        );
    }

    #[test]
    fn with_timeout_does_not_cancel_worker_on_timeout() {
        let finished = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&finished);
        let _ = with_timeout(Duration::from_millis(50), move || {
            thread::sleep(Duration::from_millis(150));
            signal.store(true, Ordering::SeqCst);
        });
        // Give the detached worker time to complete.
        thread::sleep(Duration::from_millis(300));
        assert!(
            finished.load(Ordering::SeqCst),
            "detached worker should still complete after timeout"
        );
    }

    #[test]
    fn partial_reason_wire_strings_are_snake_case() {
        assert_eq!(PartialReason::CloseTimeout.as_wire_str(), "close_timeout");
        assert_eq!(
            PartialReason::AgentCloseTimeout.as_wire_str(),
            "agent_close_timeout"
        );
        assert_eq!(
            PartialReason::SummaryTimeout.as_wire_str(),
            "summary_timeout"
        );
        assert_eq!(PartialReason::SummaryEmpty.as_wire_str(), "summary_empty");
        assert_eq!(PartialReason::BridgeTimeout.as_wire_str(), "bridge_timeout");
        assert_eq!(
            PartialReason::PersistenceError.as_wire_str(),
            "persistence_error"
        );
        // Display matches as_wire_str exactly.
        assert_eq!(format!("{}", PartialReason::CloseTimeout), "close_timeout");
    }

    #[test]
    fn partial_reason_serde_roundtrip_uses_snake_case() {
        let r = PartialReason::AgentCloseTimeout;
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(j, "\"agent_close_timeout\"");
        let back: PartialReason = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }
}
