//! The primary [`OodaThread`] — the active OODA loop fitted into the
//! cognitive-thread abstraction (design §6).
//!
//! `kind = Ooda`, `priority = Critical`, `policy = Interval(interval_secs)`.
//! Its `tick()` performs the exact current per-cycle work in the same order
//! (heartbeat → `run_ooda_cycle` → persist report/episode/health/metrics), so
//! the daemon's external cadence and side-effects are byte-for-byte preserved.
//! The `tick()` body is implemented; the OODA state/memories/
//! config it owns are moved in at construction, matching Appendix A.7/A.9.

use std::time::Instant;

use crate::ooda_loop::{OodaClients, OodaConfig, OodaPhase, OodaState};
use crate::operator_commands_ooda::persistence::{persist_cycle_report, persist_cycle_to_memory};

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
    memories: OodaClients,
    config: OodaConfig,
    interval_secs: u64,
    cycles: u64,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
}

impl OodaThread {
    /// Construct the primary thread from the daemon's OODA resources (moved in)
    /// and the configured cadence (`SIMARD_OODA_INTERVAL_SECS`).
    pub fn new(
        state: OodaState,
        memories: OodaClients,
        config: OodaConfig,
        interval_secs: u64,
    ) -> Self {
        Self {
            state,
            memories,
            config,
            interval_secs,
            cycles: 0,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
        }
    }

    /// Reclaim the owned OODA resources (for the daemon's post-loop graceful
    /// shutdown, which flushes the board and closes the session). Supports the
    /// full cutover where the daemon drives OODA solely through this thread.
    pub fn into_parts(self) -> (OodaState, OodaClients) {
        (self.state, self.memories)
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
        let start = Instant::now();
        self.cycles = self.cycles.saturating_add(1);
        self.state.cycle_start_epoch = ctx.now_epoch;

        // One complete OODA cycle, then the SAME canonical per-cycle side
        // effects the daemon performs (report + episode persistence + metrics),
        // via the shared `persist_*` helpers so there is a single source of
        // truth for the cycle's durable output.
        let outcome = match crate::ooda_loop::run_ooda_cycle(
            &mut self.state,
            &mut self.memories,
            &self.config,
        ) {
            Ok(report) => {
                let elapsed = start.elapsed();
                let summary = crate::ooda_loop::summarize_cycle_report(&report);
                self.state.last_cycle_summary = Some(summary.clone());
                self.state.last_cycle_duration_secs = Some(elapsed.as_secs());
                self.state.current_phase = OodaPhase::Sleep;

                persist_cycle_report(ctx.state_root, &report);
                persist_cycle_to_memory(&self.memories, &report);
                let _ = crate::self_metrics::collect_and_record_all(elapsed);

                self.last_success = Some(true);
                ThreadOutcome::ok(summary, elapsed).with_detail(serde_json::json!({
                    "cycle": self.cycles,
                    "cycle_number": report.cycle_number,
                }))
            }
            Err(e) => {
                let elapsed = start.elapsed();
                self.last_success = Some(false);
                ThreadOutcome::failed(format!("OODA cycle error: {e}"), elapsed)
            }
        };

        self.last_run_epoch = Some(ctx.now_epoch);
        self.next_run_epoch = super::super::schedule::next_run_epoch(
            &self.policy(),
            self.last_run_epoch,
            ctx.now_epoch,
        );
        outcome
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: OODA_ID.to_string(),
            enabled: true,
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            // OODA is never backed off (Priority::Critical); the scheduler
            // enforces this and never accrues errors against it.
            consecutive_errors: 0,
            backoff_until_epoch: None,
        }
    }
}
