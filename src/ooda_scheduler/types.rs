//! Data types for the OODA scheduler.

use std::fmt::{self, Display, Formatter};

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

/// A completed slot returned by [`poll_slots`](super::poll_slots).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::ActionKind;

    fn sample_action() -> PlannedAction {
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".to_string()),
            description: "advance goal".to_string(),
        }
    }

    fn sample_outcome(success: bool) -> ActionOutcome {
        ActionOutcome {
            action: sample_action(),
            success,
            detail: "detail".to_string(),
        }
    }

    #[test]
    fn slot_status_display_pending() {
        assert_eq!(format!("{}", SlotStatus::Pending), "pending");
    }

    #[test]
    fn slot_status_display_running() {
        let s = SlotStatus::Running { started_at: 12345 };
        assert_eq!(format!("{s}"), "running(since=12345)");
    }

    #[test]
    fn slot_status_display_completed() {
        let s = SlotStatus::Completed(sample_outcome(true));
        let d = format!("{s}");
        assert!(d.contains("completed(success=true)"));
    }

    #[test]
    fn slot_status_display_failed() {
        let s = SlotStatus::Failed("timeout".to_string());
        assert_eq!(format!("{s}"), "failed(timeout)");
    }

    #[test]
    fn scheduler_slot_construction() {
        let slot = SchedulerSlot {
            slot_id: 0,
            goal_id: "g1".to_string(),
            action: sample_action(),
            status: SlotStatus::Pending,
        };
        assert_eq!(slot.slot_id, 0);
        assert_eq!(slot.goal_id, "g1");
    }

    #[test]
    fn completed_slot_construction() {
        let cs = CompletedSlot {
            slot_id: 1,
            goal_id: "g2".to_string(),
            outcome: Ok(sample_outcome(true)),
        };
        assert!(cs.outcome.is_ok());
    }

    #[test]
    fn completed_slot_with_error() {
        let cs = CompletedSlot {
            slot_id: 2,
            goal_id: "g3".to_string(),
            outcome: Err("boom".to_string()),
        };
        assert!(cs.outcome.is_err());
    }

    #[test]
    fn scheduled_action_construction() {
        let sa = ScheduledAction {
            slot_id: 3,
            goal_id: "g4".to_string(),
            action: sample_action(),
        };
        assert_eq!(sa.slot_id, 3);
    }
}
