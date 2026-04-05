use super::*;
use crate::ooda_loop::{ActionKind, ActionOutcome, PlannedAction};

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
