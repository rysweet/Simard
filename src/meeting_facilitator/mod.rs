//! Interactive meeting mode — structured session with decisions and action items.
//!
//! `MeetingSession` captures a running meeting with its topic, decisions made,
//! and action items assigned. The facilitator stores a durable summary into
//! cognitive memory when the meeting closes.

mod handoff;
mod session;
mod types;

// Re-export all public items so `crate::meeting_facilitator::X` still works.
pub use handoff::{
    MEETING_HANDOFF_FILENAME, MEETING_SESSION_WIP_FILENAME, MeetingHandoff, default_handoff_dir,
    load_meeting_handoff, load_session_wip, mark_handoff_processed_in_place,
    mark_meeting_handoff_processed, remove_session_wip, save_session_wip, write_meeting_handoff,
};
pub use session::{
    add_note, add_question, close_meeting, edit_item, record_action_item, record_decision,
    remove_item, start_meeting,
};
pub use types::{ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus, OpenQuestion};
