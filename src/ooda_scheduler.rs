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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::ActionKind;

    fn make_action(goal_id: Option<&str>) -> PlannedAction {
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: goal_id.map(String::from),
            description: "test action".to_string(),
        }
    }

    fn make_outcome(success: bool) -> ActionOutcome {
        ActionOutcome {
            action: make_action(None),
            success,
            detail: "done".to_string(),
        }
    }

    // 1. Scheduler::new sets max_concurrent, starts empty
    #[test]
    fn new_scheduler_is_empty() {
        let s = Scheduler::new(4);
        assert_eq!(s.max_concurrent, 4);
        assert!(s.slots.is_empty());
        assert_eq!(s.active_count(), 0);
        assert!(s.has_capacity());
    }

    // 2. schedule_actions respects max_concurrent capacity
    #[test]
    fn schedule_actions_respects_capacity() {
        let mut s = Scheduler::new(2);
        let actions = vec![
            make_action(Some("g1")),
            make_action(Some("g2")),
            make_action(Some("g3")),
        ];
        let scheduled = schedule_actions(&mut s, actions).unwrap();
        assert_eq!(scheduled.len(), 2);
        assert_eq!(s.slots.len(), 2);
        assert_eq!(s.active_count(), 2);
        assert!(!s.has_capacity());
    }

    // 3. schedule_actions assigns sequential slot IDs
    #[test]
    fn schedule_actions_assigns_sequential_ids() {
        let mut s = Scheduler::new(10);
        let batch1 = vec![make_action(None), make_action(None)];
        let scheduled1 = schedule_actions(&mut s, batch1).unwrap();
        assert_eq!(scheduled1[0].slot_id, 0);
        assert_eq!(scheduled1[1].slot_id, 1);

        let batch2 = vec![make_action(None)];
        let scheduled2 = schedule_actions(&mut s, batch2).unwrap();
        assert_eq!(scheduled2[0].slot_id, 2);
    }

    // 4a. start_slot transitions Pending → Running
    #[test]
    fn start_slot_pending_to_running() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(Some("g1"))]).unwrap();
        start_slot(&mut s, 0).unwrap();
        assert!(matches!(s.slots[0].status, SlotStatus::Running { .. }));
    }

    // 4b. start_slot errors on missing slot
    #[test]
    fn start_slot_missing_slot_errors() {
        let mut s = Scheduler::new(2);
        let result = start_slot(&mut s, 99);
        assert!(result.is_err());
    }

    // 4c. start_slot errors on non-pending (already running) slot
    #[test]
    fn start_slot_non_pending_errors() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        start_slot(&mut s, 0).unwrap();
        let result = start_slot(&mut s, 0);
        assert!(result.is_err());
    }

    // 5a. complete_slot only works on Running slots
    #[test]
    fn complete_slot_on_running_succeeds() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        start_slot(&mut s, 0).unwrap();
        complete_slot(&mut s, 0, make_outcome(true)).unwrap();
        assert!(matches!(s.slots[0].status, SlotStatus::Completed(_)));
    }

    #[test]
    fn complete_slot_on_pending_errors() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        assert!(complete_slot(&mut s, 0, make_outcome(true)).is_err());
    }

    #[test]
    fn complete_slot_missing_slot_errors() {
        let mut s = Scheduler::new(2);
        assert!(complete_slot(&mut s, 42, make_outcome(true)).is_err());
    }

    // 5b. fail_slot only works on Running slots
    #[test]
    fn fail_slot_on_running_succeeds() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        start_slot(&mut s, 0).unwrap();
        fail_slot(&mut s, 0, "boom".to_string()).unwrap();
        assert!(matches!(s.slots[0].status, SlotStatus::Failed(_)));
    }

    #[test]
    fn fail_slot_on_pending_errors() {
        let mut s = Scheduler::new(2);
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        assert!(fail_slot(&mut s, 0, "nope".to_string()).is_err());
    }

    #[test]
    fn fail_slot_missing_slot_errors() {
        let mut s = Scheduler::new(2);
        assert!(fail_slot(&mut s, 42, "nope".to_string()).is_err());
    }

    // 6. poll_slots returns completed/failed without removing
    #[test]
    fn poll_slots_returns_finished_without_removing() {
        let mut s = Scheduler::new(4);
        schedule_actions(
            &mut s,
            vec![
                make_action(Some("a")),
                make_action(Some("b")),
                make_action(Some("c")),
            ],
        )
        .unwrap();

        start_slot(&mut s, 0).unwrap();
        start_slot(&mut s, 1).unwrap();
        complete_slot(&mut s, 0, make_outcome(true)).unwrap();
        fail_slot(&mut s, 1, "err".to_string()).unwrap();

        let polled = poll_slots(&mut s);
        assert_eq!(polled.len(), 2);
        assert!(polled[0].outcome.is_ok());
        assert!(polled[1].outcome.is_err());
        // Slots are still present
        assert_eq!(s.slots.len(), 3);
    }

    // 7. drain_finished removes completed/failed slots
    #[test]
    fn drain_finished_removes_completed_and_failed() {
        let mut s = Scheduler::new(4);
        schedule_actions(
            &mut s,
            vec![
                make_action(Some("a")),
                make_action(Some("b")),
                make_action(Some("c")),
            ],
        )
        .unwrap();

        start_slot(&mut s, 0).unwrap();
        start_slot(&mut s, 1).unwrap();
        complete_slot(&mut s, 0, make_outcome(true)).unwrap();
        fail_slot(&mut s, 1, "err".to_string()).unwrap();

        let drained = drain_finished(&mut s);
        assert_eq!(drained.len(), 2);
        // Only the pending slot remains
        assert_eq!(s.slots.len(), 1);
        assert!(matches!(s.slots[0].status, SlotStatus::Pending));
    }

    // 8. scheduler_summary contains slot counts
    #[test]
    fn scheduler_summary_contains_counts() {
        let mut s = Scheduler::new(4);
        schedule_actions(&mut s, vec![make_action(None), make_action(None)]).unwrap();
        start_slot(&mut s, 0).unwrap();

        let summary = scheduler_summary(&s);
        assert!(summary.contains("1 pending"));
        assert!(summary.contains("1 running"));
        assert!(summary.contains("0 completed"));
        assert!(summary.contains("0 failed"));
        assert!(summary.contains("max=4"));
    }

    // 9. has_capacity returns false when at limit
    #[test]
    fn has_capacity_false_at_limit() {
        let mut s = Scheduler::new(1);
        assert!(s.has_capacity());
        schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        assert!(!s.has_capacity());
    }

    // Extra: goal_id defaults to empty string when None
    #[test]
    fn goal_id_defaults_to_empty_when_none() {
        let mut s = Scheduler::new(2);
        let scheduled = schedule_actions(&mut s, vec![make_action(None)]).unwrap();
        assert_eq!(scheduled[0].goal_id, "");
        assert_eq!(s.slots[0].goal_id, "");
    }

    // Extra: drain on empty scheduler returns nothing
    #[test]
    fn drain_empty_scheduler() {
        let mut s = Scheduler::new(2);
        let drained = drain_finished(&mut s);
        assert!(drained.is_empty());
    }
}
