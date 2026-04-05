mod evidence;
mod execution;
mod parsing;
mod session;
mod types;
mod workflow_guard;

// Re-export all public items so `crate::terminal_session::X` still works.
pub(crate) use evidence::{
    compact_terminal_evidence_value, render_terminal_step, transcript_preview,
};
pub use execution::execute_terminal_turn;
pub(crate) use execution::resolve_working_directory;
pub(crate) use session::PtyTerminalSession;
pub(crate) use types::{TerminalSessionCapture, TerminalStep};
