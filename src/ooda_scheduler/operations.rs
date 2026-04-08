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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::{ActionKind, ActionOutcome, PlannedAction};

    fn make_action(goal_id: Option<&str>) -> PlannedAction {
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: goal_id.map(|s| s.to_string()),
            description: "test action".to_string(),
        }
    }

    fn make_outcome(action: PlannedAction) -> ActionOutcome {
        ActionOutcome {
            action,
            success: true,
            detail: "done".to_string(),
        }
    }

    #[test]
    fn test_schedule_actions_basic() {
        let mut sched = Scheduler::new(5);
        let actions = vec![make_action(Some("g1")), make_action(Some("g2"))];
        let scheduled = schedule_actions(&mut sched, actions).unwrap();
        assert_eq!(scheduled.len(), 2);
        assert_eq!(scheduled[0].slot_id, 0);
        assert_eq!(scheduled[1].slot_id, 1);
        assert_eq!(sched.slots.len(), 2);
    }

    #[test]
    fn test_schedule_actions_empty() {
        let mut sched = Scheduler::new(5);
        let scheduled = schedule_actions(&mut sched, vec![]).unwrap();
        assert!(scheduled.is_empty());
    }

    #[test]
    fn test_schedule_actions_respects_capacity() {
        let mut sched = Scheduler::new(1);
        // Fill capacity with a pending slot
        let actions = vec![make_action(Some("g1")), make_action(Some("g2"))];
        let scheduled = schedule_actions(&mut sched, actions).unwrap();
        // max_concurrent = 1, active_count counts pending+running
        // With has_capacity checking active_count < max_concurrent, only 1 should be scheduled
        assert_eq!(scheduled.len(), 1);
    }

    #[test]
    fn test_start_slot_transitions_pending_to_running() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        start_slot(&mut sched, 0).unwrap();
        assert!(matches!(sched.slots[0].status, SlotStatus::Running { .. }));
    }

    #[test]
    fn test_start_slot_not_found() {
        let mut sched = Scheduler::new(5);
        let result = start_slot(&mut sched, 999);
        assert!(result.is_err());
    }

    #[test]
    fn test_start_slot_not_pending() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        start_slot(&mut sched, 0).unwrap();
        // Starting an already running slot should fail
        let result = start_slot(&mut sched, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_complete_slot_success() {
        let mut sched = Scheduler::new(5);
        let actions = vec![make_action(Some("g"))];
        schedule_actions(&mut sched, actions).unwrap();
        start_slot(&mut sched, 0).unwrap();

        let outcome = make_outcome(make_action(Some("g")));
        complete_slot(&mut sched, 0, outcome).unwrap();
        assert!(matches!(sched.slots[0].status, SlotStatus::Completed(_)));
    }

    #[test]
    fn test_complete_slot_not_running() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        // Slot is pending, not running
        let outcome = make_outcome(make_action(None));
        let result = complete_slot(&mut sched, 0, outcome);
        assert!(result.is_err());
    }

    #[test]
    fn test_fail_slot_success() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        start_slot(&mut sched, 0).unwrap();

        fail_slot(&mut sched, 0, "something broke".to_string()).unwrap();
        assert!(matches!(sched.slots[0].status, SlotStatus::Failed(_)));
    }

    #[test]
    fn test_fail_slot_not_running() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        let result = fail_slot(&mut sched, 0, "err".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_poll_slots_returns_completed_and_failed() {
        let mut sched = Scheduler::new(5);
        schedule_actions(
            &mut sched,
            vec![make_action(Some("a")), make_action(Some("b"))],
        )
        .unwrap();

        start_slot(&mut sched, 0).unwrap();
        start_slot(&mut sched, 1).unwrap();

        let outcome = make_outcome(make_action(Some("a")));
        complete_slot(&mut sched, 0, outcome).unwrap();
        fail_slot(&mut sched, 1, "timeout".to_string()).unwrap();

        let polled = poll_slots(&mut sched);
        assert_eq!(polled.len(), 2);
        assert!(polled[0].outcome.is_ok());
        assert!(polled[1].outcome.is_err());
    }

    #[test]
    fn test_poll_slots_ignores_pending_and_running() {
        let mut sched = Scheduler::new(5);
        schedule_actions(&mut sched, vec![make_action(None), make_action(None)]).unwrap();
        start_slot(&mut sched, 1).unwrap();

        let polled = poll_slots(&mut sched);
        assert!(polled.is_empty());
    }

    #[test]
    fn test_drain_finished_removes_completed() {
        let mut sched = Scheduler::new(5);
        schedule_actions(
            &mut sched,
            vec![make_action(Some("a")), make_action(Some("b"))],
        )
        .unwrap();

        start_slot(&mut sched, 0).unwrap();
        let outcome = make_outcome(make_action(Some("a")));
        complete_slot(&mut sched, 0, outcome).unwrap();

        let finished = drain_finished(&mut sched);
        assert_eq!(finished.len(), 1);
        // Only the pending slot remains
        assert_eq!(sched.slots.len(), 1);
        assert!(matches!(sched.slots[0].status, SlotStatus::Pending));
    }

    #[test]
    fn test_scheduler_summary_format() {
        let mut sched = Scheduler::new(3);
        schedule_actions(&mut sched, vec![make_action(None)]).unwrap();
        let summary = scheduler_summary(&sched);
        assert!(summary.contains("1 pending"));
        assert!(summary.contains("0 running"));
        assert!(summary.contains("max=3"));
    }

    #[test]
    fn test_schedule_actions_goal_id_defaults_to_empty() {
        let mut sched = Scheduler::new(5);
        let actions = vec![make_action(None)];
        let scheduled = schedule_actions(&mut sched, actions).unwrap();
        assert_eq!(scheduled[0].goal_id, "");
    }
}
