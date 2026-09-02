//! Live outcome signals for closed-loop verification (issue #2751).
//!
//! The framing invariant: an **artifact** (a merged PR, a deploy) is NOT an
//! **outcome**. A goal is "achieved" only once a verified LIVE signal
//! corroborates its real success criteria in production. This module carries
//! the signal-acquisition half of that loop: [`LiveSignal`] (one authenticated
//! observation) and [`LiveSignalSource`] (the injected gatherer).
//!
//! The load-bearing safety control — "never mark achieved without ≥1 verified
//! live signal" — lives in the Rust rail in
//! [`outcome_verify`](super::outcome_verify), NOT in any prompt. The
//! per-signal `verified` flag below is set exclusively by an adapter that
//! authenticated the observation (a crossed metric threshold, a matched
//! journald line, a cleared deploy drift, a successful behavior re-probe); it
//! is NEVER derived from model output. That is what makes the rail
//! non-bypassable by prompt injection or recipe tampering.
//!
//! Lookups are injected through [`LiveSignalSource`] (mirroring
//! [`EvidenceSource`](super::completion_gate::EvidenceSource)) so the
//! verification logic is pure and runs hermetically with no network, no live
//! `gh`, and no `journalctl`.

use chrono::{DateTime, Utc};

use crate::error::SimardResult;

use super::types::ActiveGoal;

/// One authenticated observation of a goal's real-world effect. Ephemeral —
/// gathered fresh each cycle, never persisted raw.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSignal {
    /// Trusted origin of the observation, e.g. `"self_metrics"`, `"journald"`,
    /// `"reconcile_detector"`, `"behavior_probe"`.
    pub source: String,
    /// What was observed, e.g. `"e2big_absent"`, `"threshold_crossed"`,
    /// `"drift_cleared"`.
    pub kind: String,
    /// The load-bearing flag. `true` ONLY when the adapter corroborated the
    /// effect from an authenticated source. NEVER set from LLM output or
    /// unsanitized text — the outcome-verify Rail-3 reads this and nothing else.
    pub verified: bool,
    /// Short, human-readable detail. Sanitized (control/ANSI-stripped, capped)
    /// before it is ever rendered into a reasoner prompt.
    pub detail: String,
    /// When the observation was made.
    pub observed_at: DateTime<Utc>,
}

/// The signal-acquisition trait. Mirrors
/// [`EvidenceSource`](super::completion_gate::EvidenceSource): lookups are
/// injected so tests run hermetically with no network, no live `gh`, and no
/// `journalctl`.
pub trait LiveSignalSource: Send + Sync {
    /// Gather every live signal relevant to this goal's real success criteria.
    ///
    /// Each adapter sets [`LiveSignal::verified`] only from an authenticated
    /// positive corroboration. On a hard error (adapter unavailable, timeout,
    /// `journalctl` failure), returns `Err` — the outcome-verify seam surfaces
    /// it as a NO-FALLBACK cycle failure, never an empty "no signals" success.
    fn gather(&self, goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>>;
}

/// Blanket impl so `&T` (and, transitively, `Arc<T>` via deref) are also
/// sources — mirrors the `EvidenceSource for &T` impl so the daemon can store
/// one `Arc<dyn LiveSignalSource>` on `OodaClients` and pass it by reference
/// into the verifier each cycle.
impl<T: LiveSignalSource + ?Sized> LiveSignalSource for &T {
    fn gather(&self, goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>> {
        (**self).gather(goal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::GoalProgress;

    fn goal() -> ActiveGoal {
        ActiveGoal::new("g", "eliminate E2BIG", 1)
    }

    struct Fixed(Vec<LiveSignal>);
    impl LiveSignalSource for Fixed {
        fn gather(&self, _goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn blanket_ref_impl_forwards_to_inner() {
        let sig = LiveSignal {
            source: "self_metrics".into(),
            kind: "threshold_crossed".into(),
            verified: true,
            detail: "p95 < 200ms".into(),
            observed_at: Utc::now(),
        };
        let src = Fixed(vec![sig.clone()]);
        // Exercise the `&T` blanket impl explicitly.
        let via_ref: &dyn LiveSignalSource = &&src;
        let out = via_ref.gather(&goal()).unwrap();
        assert_eq!(out, vec![sig]);
    }

    #[test]
    fn live_signal_is_clone_debug_partialeq() {
        let a = LiveSignal {
            source: "journald".into(),
            kind: "e2big_absent".into(),
            verified: false,
            detail: "not yet observed".into(),
            observed_at: Utc::now(),
        };
        let b = a.clone();
        assert_eq!(a, b);
        assert!(format!("{a:?}").contains("journald"));
    }

    #[test]
    fn goal_progress_import_is_used() {
        // Keep the GoalProgress import meaningful without over-fixturing.
        let mut g = goal();
        g.status = GoalProgress::Completed;
        assert!(matches!(g.status, GoalProgress::Completed));
    }
}
