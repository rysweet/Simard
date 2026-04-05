//! Scheduler for concurrent OODA actions.
//!
//! The [`Scheduler`] manages a fixed pool of [`SchedulerSlot`]s that track
//! the lifecycle of dispatched actions from pending through running to
//! completed or failed. The OODA loop uses this to enforce its concurrency
//! limit and collect results.

mod operations;
#[cfg(test)]
mod tests;
mod types;

// Re-export all public items so `crate::ooda_scheduler::X` still works.
pub use operations::{
    complete_slot, drain_finished, fail_slot, poll_slots, schedule_actions, scheduler_summary,
    start_slot,
};
pub use types::{CompletedSlot, ScheduledAction, SchedulerSlot, SlotStatus};

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Manages concurrent action slots for the OODA loop.
///
/// The scheduler enforces a maximum concurrency limit. Actions are queued
/// as `Pending` and must be explicitly transitioned through `Running` to
/// `Completed` or `Failed`.
pub struct Scheduler {
    pub slots: Vec<SchedulerSlot>,
    pub max_concurrent: u32,
    pub(crate) next_slot_id: usize,
}

impl Scheduler {
    /// Create a new scheduler with the given concurrency limit.
    pub fn new(max_concurrent: u32) -> Self {
        Self {
            slots: Vec::new(),
            max_concurrent,
            next_slot_id: 0,
        }
    }

    /// How many slots are currently in `Pending` or `Running` state.
    pub fn active_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s.status, SlotStatus::Pending | SlotStatus::Running { .. }))
            .count()
    }

    /// Whether the scheduler can accept more actions.
    pub fn has_capacity(&self) -> bool {
        self.active_count() < self.max_concurrent as usize
    }
}
