//! Scheduler for concurrent OODA actions.
//!
//! The [`Scheduler`] manages a fixed pool of [`SchedulerSlot`]s that track
//! the lifecycle of dispatched actions from pending through running to
//! completed or failed. The OODA loop uses this to enforce its concurrency
//! limit and collect results.

use std::fmt::{self, Display, Formatter};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::{ActionOutcome, PlannedAction};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Status of a single scheduler slot.
#[derive(Clone, Debug)]
pub enum SlotStatus {
    /// Action is queued but not yet started.
    Pending,
    /// Action is currently executing.
    Running {
        /// Unix epoch seconds when the action started.
        started_at: u64,
    },
    /// Action finished successfully.
    Completed(ActionOutcome),
    /// Action failed with an error message.
    Failed(String),
}

impl Display for SlotStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => f.write_str("pending"),
            Self::Running { started_at } => write!(f, "running(since={started_at})"),
            Self::Completed(outcome) => {
                write!(f, "completed(success={})", outcome.success)
            }
            Self::Failed(reason) => write!(f, "failed({reason})"),
        }
    }
}

/// A single slot in the scheduler.
#[derive(Clone, Debug)]
pub struct SchedulerSlot {
    pub slot_id: usize,
    pub goal_id: String,
    pub action: PlannedAction,
    pub status: SlotStatus,
}

/// A completed slot returned by [`poll_slots`].
#[derive(Clone, Debug)]
pub struct CompletedSlot {
    pub slot_id: usize,
    pub goal_id: String,
    pub outcome: Result<ActionOutcome, String>,
}

/// A reference to a scheduled action.
#[derive(Clone, Debug)]
pub struct ScheduledAction {
    pub slot_id: usize,
    pub goal_id: String,
    pub action: PlannedAction,
}

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
    next_slot_id: usize,
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

/// Schedule a batch of actions, assigning each to a slot.
///
/// Actions that exceed the concurrency limit are rejected. Returns the
/// successfully scheduled actions.
pub fn schedule_actions(
    scheduler: &mut Scheduler,
    actions: Vec<PlannedAction>,
) -> SimardResult<Vec<ScheduledAction>> {
    let mut scheduled = Vec::new();

    for action in actions {
        if !scheduler.has_capacity() {
            break;
        }

        let slot_id = scheduler.next_slot_id;
        scheduler.next_slot_id += 1;

        let goal_id = action.goal_id.clone().unwrap_or_default();

        let slot = SchedulerSlot {
            slot_id,
            goal_id: goal_id.clone(),
            action: action.clone(),
            status: SlotStatus::Pending,
        };
        scheduler.slots.push(slot);

        scheduled.push(ScheduledAction {
            slot_id,
            goal_id,
            action,
        });
    }

    Ok(scheduled)
}

/// Transition a pending slot to running.
pub fn start_slot(scheduler: &mut Scheduler, slot_id: usize) -> SimardResult<()> {
    let slot = scheduler
        .slots
        .iter_mut()
        .find(|s| s.slot_id == slot_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "slot_id".to_string(),
            reason: format!("slot {slot_id} not found"),
        })?;

    if !matches!(slot.status, SlotStatus::Pending) {
        return Err(SimardError::InvalidGoalRecord {
            field: "slot_status".to_string(),
            reason: format!("slot {slot_id} is not pending, current: {}", slot.status),
        });
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        })?
        .as_secs();

    slot.status = SlotStatus::Running { started_at: now };
    Ok(())
}

/// Mark a running slot as completed with an outcome.
pub fn complete_slot(
    scheduler: &mut Scheduler,
    slot_id: usize,
    outcome: ActionOutcome,
) -> SimardResult<()> {
    let slot = scheduler
        .slots
        .iter_mut()
        .find(|s| s.slot_id == slot_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "slot_id".to_string(),
            reason: format!("slot {slot_id} not found"),
        })?;

    if !matches!(slot.status, SlotStatus::Running { .. }) {
        return Err(SimardError::InvalidGoalRecord {
            field: "slot_status".to_string(),
            reason: format!("slot {slot_id} is not running, current: {}", slot.status),
        });
    }

    slot.status = SlotStatus::Completed(outcome);
    Ok(())
}

/// Mark a running slot as failed.
pub fn fail_slot(scheduler: &mut Scheduler, slot_id: usize, reason: String) -> SimardResult<()> {
    let slot = scheduler
        .slots
        .iter_mut()
        .find(|s| s.slot_id == slot_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "slot_id".to_string(),
            reason: format!("slot {slot_id} not found"),
        })?;

    if !matches!(slot.status, SlotStatus::Running { .. }) {
        return Err(SimardError::InvalidGoalRecord {
            field: "slot_status".to_string(),
            reason: format!("slot {slot_id} is not running, current: {}", slot.status),
        });
    }

    slot.status = SlotStatus::Failed(reason);
    Ok(())
}

/// Poll all slots and return those that have completed or failed.
///
/// The returned slots are *not* removed from the scheduler so their
/// state can be inspected later. Callers should use the `CompletedSlot`
/// to decide whether to retry failed actions.
pub fn poll_slots(scheduler: &mut Scheduler) -> Vec<CompletedSlot> {
    let mut results = Vec::with_capacity(scheduler.slots.len());
    for slot in &scheduler.slots {
        match &slot.status {
            SlotStatus::Completed(outcome) => results.push(CompletedSlot {
                slot_id: slot.slot_id,
                goal_id: slot.goal_id.clone(),
                outcome: Ok(outcome.clone()),
            }),
            SlotStatus::Failed(reason) => results.push(CompletedSlot {
                slot_id: slot.slot_id,
                goal_id: slot.goal_id.clone(),
                outcome: Err(reason.clone()),
            }),
            _ => {}
        }
    }
    results
}

/// Remove all completed and failed slots from the scheduler.
pub fn drain_finished(scheduler: &mut Scheduler) -> Vec<CompletedSlot> {
    let mut finished = Vec::with_capacity(scheduler.slots.len());
    scheduler.slots.retain(|slot| match &slot.status {
        SlotStatus::Completed(outcome) => {
            finished.push(CompletedSlot {
                slot_id: slot.slot_id,
                goal_id: slot.goal_id.clone(),
                outcome: Ok(outcome.clone()),
            });
            false
        }
        SlotStatus::Failed(reason) => {
            finished.push(CompletedSlot {
                slot_id: slot.slot_id,
                goal_id: slot.goal_id.clone(),
                outcome: Err(reason.clone()),
            });
            false
        }
        _ => true,
    });
    finished
}

/// Summary of the scheduler state for logging.
pub fn scheduler_summary(scheduler: &Scheduler) -> String {
    let (mut pending, mut running, mut completed, mut failed) = (0usize, 0usize, 0usize, 0usize);
    for slot in &scheduler.slots {
        match &slot.status {
            SlotStatus::Pending => pending += 1,
            SlotStatus::Running { .. } => running += 1,
            SlotStatus::Completed(_) => completed += 1,
            SlotStatus::Failed(_) => failed += 1,
        }
    }
    format!(
        "Scheduler: {pending} pending, {running} running, {completed} completed, {failed} failed (max={})",
        scheduler.max_concurrent
    )
}
