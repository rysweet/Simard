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
mod types;

#[cfg(test)]
mod tests;

/// Maximum retries per goal before the supervisor gives up.
const MAX_RETRIES_PER_GOAL: u32 = 2;

/// Seconds after which a subordinate is considered stale.
const STALE_THRESHOLD_SECONDS: u64 = 120;

// Re-export all public items so `crate::agent_supervisor::X` still works.
pub use lifecycle::{check_heartbeat, is_goal_complete, kill_subordinate, spawn_subordinate};
pub use types::{
    HeartbeatStatus, SubordinateConfig, SubordinateHandle, max_retries_per_goal,
    stale_threshold_seconds,
};
