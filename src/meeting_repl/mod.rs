//! Interactive meeting REPL — a **conversation** with Simard that also captures
//! decisions, action items, and notes.
//!
//! Natural-language lines are sent to the active base type agent (RustyClawd,
//! Copilot, Claude CLI, etc.) via `run_turn`. The agent's text response is
//! displayed and also recorded as a meeting note so the transcript is preserved.
//! Structured slash-commands (`/decision`, `/action`, `/note`, `/close`) bypass
//! the agent and record directly.
//!
//! The REPL produces a durable `MeetingSession` (with `MeetingRecord` summary)
//! when the operator types `/close` or stdin reaches EOF.

mod auto_capture;
mod command;
mod persist;
mod repl;
#[cfg(test)]
mod test_support;

// Re-export all public items so `crate::meeting_repl::X` still works.
pub use command::{MeetingCommand, parse_meeting_command};
pub use repl::run_meeting_repl;
