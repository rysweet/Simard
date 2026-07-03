//! The primary [`OodaThread`] — the active OODA loop fitted into the
//! cognitive-thread abstraction (design §6).
//!
//! `kind = Ooda`, `priority = Critical`, `policy = Interval(interval_secs)`.
//! Its `tick()` performs the exact current per-cycle work in the same order
//! (heartbeat → `run_ooda_cycle` → persist report/episode/health/metrics), so
//! the daemon's external cadence and side-effects are byte-for-byte preserved.
//! The `tick()` body is a `todo!()` stub during TDD; the OODA state/bridges/
//! config it owns are moved in at construction, matching Appendix A.7/A.9.
#![allow(dead_code, unused_variables)]

use crate::ooda_loop::{OodaBridges, OodaConfig, OodaState};

use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

/// Stable telemetry id for the primary loop.
const OODA_ID: &str = "ooda";

/// The primary cognitive thread: owns the mutable OODA state and drives one
/// `run_ooda_cycle` per tick.
pub struct OodaThread {
    state: OodaState,
    bridges: OodaBridges,
    config: OodaConfig,
    interval_secs: u64,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
}

impl OodaThread {
    /// Construct the primary thread from the daemon's OODA resources (moved in)
    /// and the configured cadence (`SIMARD_OODA_INTERVAL_SECS`).
    pub fn new(
        state: OodaState,
        bridges: OodaBridges,
        config: OodaConfig,
        interval_secs: u64,
    ) -> Self {
        Self {
            state,
            bridges,
            config,
            interval_secs,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
        }
    }
}

impl CognitiveThread for OodaThread {
    fn id(&self) -> &str {
        OODA_ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Ooda
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(std::time::Duration::from_secs(self.interval_secs))
    }

    fn priority(&self) -> Priority {
        Priority::Critical
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        todo!("Step 7 TDD: OODA-as-thread body implemented by the implementation step")
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: OODA_ID.to_string(),
            enabled: true,
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: 0,
            backoff_until_epoch: None,
        }
    }
}
