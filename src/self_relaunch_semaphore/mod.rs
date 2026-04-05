//! File-based leader semaphore and coordinated handoff for self-relaunch.
//!
//! Provides a `LeaderSemaphore` backed by a lock file with PID ownership,
//! heartbeat timestamps, and a monotonic generation counter. The
//! `LeaderHandoff` orchestrator coordinates: build canary → verify gates →
//! spawn child → confirm healthy → transfer leadership → old exits.

mod handoff;
pub(crate) mod semaphore;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::self_relaunch_semaphore::X` still works.
pub use handoff::{HandoffConfig, HandoffResult, coordinated_handoff, signal_ready};
pub use semaphore::{LeaderSemaphore, LeaderState};
