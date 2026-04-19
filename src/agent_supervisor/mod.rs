//! Subordinate lifecycle management for agent composition.
//!
//! The supervisor spawns subordinate agents as child processes, tracks their
//! liveness via heartbeats stored in the hive (cognitive memory), and can
//! kill them when they become unresponsive or complete their goals.
//!
//! Each subordinate gets its own `agent_name` for memory isolation per
//! Pillar 8 (Identity != Runtime). Communication is exclusively through
//! semantic facts in the hive, never raw IPC.

mod lifecycle;
pub mod tmux;
mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_tmux;

/// Maximum retries per goal before the supervisor gives up.
const MAX_RETRIES_PER_GOAL: u32 = 5;

/// Seconds after which a subordinate is considered stale.
/// Agent sessions routinely take 10-30 minutes per step — 120s was far too
/// aggressive and caused false stale detections. 30 minutes is reasonable.
const STALE_THRESHOLD_SECONDS: u64 = 1800;

// Re-export all public items so `crate::agent_supervisor::X` still works.
pub use lifecycle::{
    check_heartbeat, count_commits_since, count_open_prs, is_goal_complete, kill_subordinate,
    reap_zombies, spawn_subordinate, validate_subordinate_artifacts,
};
pub use types::{
    HeartbeatStatus, SubordinateConfig, SubordinateHandle, max_retries_per_goal,
    stale_threshold_seconds,
};
