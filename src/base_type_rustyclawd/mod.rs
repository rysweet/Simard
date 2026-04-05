//! RustyClawd agent base type — wraps the rustyclawd-core SDK to delegate
//! objectives through its LLM-calling, tool-executing agent pipeline.
//!
//! This is the primary production base type. It uses `rustyclawd_core::Client`
//! with `execute_with_tools` to run a full agent loop: Simard sends an objective,
//! RustyClawd calls Claude, Claude requests tool use, tools execute locally, and
//! the loop continues until the objective is resolved.
//!
//! When no API key is available, falls back to spawning the `rustyclawd` binary
//! as a subprocess.

mod adapter;
mod execution;
mod session;
mod tool_executor;
mod tools;

/// Maximum conversation messages to retain in history. Keep low because
/// each tool-use turn can be very large (tool inputs + outputs).
const MAX_HISTORY_MESSAGES: usize = 20;

pub use adapter::RustyClawdAdapter;
