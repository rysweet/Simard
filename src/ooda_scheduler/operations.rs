//! Slot lifecycle operations for the OODA scheduler.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::{ActionOutcome, PlannedAction};

use super::Scheduler;
use super::types::{CompletedSlot, ScheduledAction, SchedulerSlot, SlotStatus};

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
