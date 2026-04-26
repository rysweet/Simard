//! Interactive meeting REPL — a thin stdin/stdout loop over `MeetingBackend`.
//!
//! All meeting intelligence lives in `meeting_backend`. This module provides
//! the CLI-specific REPL loop and backward-compatible re-exports.

mod color;
mod repl;
mod spinner;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_repl;

pub use color::{cyan, green, yellow};
pub use repl::run_meeting_repl;

// Backward-compatible re-exports: the old `MeetingCommand` and
// `parse_meeting_command` from `meeting_backend::command`.
pub use crate::meeting_backend::command::MeetingCommand;
pub use crate::meeting_backend::command::parse_command as parse_meeting_command;
