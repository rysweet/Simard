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
