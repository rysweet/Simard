//! Types for subordinate agent management.

use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use crate::agent_roles::AgentRole;
use crate::error::{SimardError, SimardResult};
use crate::identity_composition::max_subordinate_depth;

use super::{MAX_RETRIES_PER_GOAL, STALE_THRESHOLD_SECONDS};

/// Configuration for spawning a subordinate agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubordinateConfig {
    /// Unique name for this subordinate (used as agent_name).
    pub agent_name: String,
    /// The goal this subordinate should pursue.
    pub goal: String,
    /// The role this subordinate fills.
    pub role: AgentRole,
    /// Working directory (typically a git worktree) for the subordinate.
    pub worktree_path: PathBuf,
    /// Current recursion depth (0 = top-level supervisor).
    pub current_depth: u32,
}

impl SubordinateConfig {
    /// Validate the configuration before spawning.
    pub fn validate(&self) -> SimardResult<()> {
        if self.agent_name.is_empty() {
            return Err(SimardError::InvalidIdentityComposition {
                identity: self.agent_name.clone(),
                reason: "subordinate agent_name cannot be empty".to_string(),
            });
        }
        if self.goal.is_empty() {
            return Err(SimardError::InvalidIdentityComposition {
                identity: self.agent_name.clone(),
                reason: "subordinate goal cannot be empty".to_string(),
            });
        }
        let depth_limit = max_subordinate_depth();
        if self.current_depth >= depth_limit {
            return Err(SimardError::InvalidIdentityComposition {
                identity: self.agent_name.clone(),
                reason: format!(
                    "subordinate depth {} would exceed max depth {} (SIMARD_MAX_SUBORDINATE_DEPTH)",
                    self.current_depth + 1,
                    depth_limit
                ),
            });
        }
        Ok(())
    }
}

/// Handle to a running subordinate process.
///
/// The handle tracks the subordinate's process ID (or a synthetic ID in
/// test mode), its goal, and retry state. The supervisor uses this to
/// check heartbeats and kill subordinates.
#[derive(Clone, Debug)]
pub struct SubordinateHandle {
    /// Process ID of the subordinate (0 for mock/test handles).
    pub pid: u32,
    /// The subordinate's unique agent name.
    pub agent_name: String,
    /// The goal this subordinate is pursuing.
    pub goal: String,
    /// Working directory for the subordinate.
    pub worktree_path: PathBuf,
    /// When the subordinate was spawned (unix epoch seconds).
    pub spawn_time: u64,
    /// How many times this goal has been retried.
    pub retry_count: u32,
    /// Whether the subordinate has been killed.
    pub killed: bool,
}

impl Display for SubordinateHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SubordinateHandle(pid={}, agent={}, retries={}, killed={})",
            self.pid, self.agent_name, self.retry_count, self.killed
        )
    }
}

impl SubordinateHandle {
    /// Whether this handle can be retried (under the max retry limit).
    pub fn can_retry(&self) -> bool {
        self.retry_count < MAX_RETRIES_PER_GOAL
    }

    /// Increment the retry counter and return the new count.
    pub fn record_retry(&mut self) -> u32 {
        self.retry_count += 1;
        self.retry_count
    }
}

/// Heartbeat status of a subordinate.
///
/// Determined by polling the subordinate's progress facts from the hive
/// and comparing timestamps against staleness thresholds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeartbeatStatus {
    /// Subordinate has reported recently.
    Alive {
        /// The last reported epoch timestamp.
        last_epoch: u64,
        /// The subordinate's current session phase.
        phase: String,
    },
    /// Subordinate has not reported within the staleness window.
    Stale {
        /// Seconds since the last heartbeat.
        seconds_since: u64,
    },
    /// No heartbeat has ever been received.
    Dead,
}

impl Display for HeartbeatStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Alive { last_epoch, phase } => {
                write!(f, "alive(epoch={last_epoch}, phase={phase})")
            }
            Self::Stale { seconds_since } => {
                write!(f, "stale(seconds_since={seconds_since})")
            }
            Self::Dead => f.write_str("dead"),
        }
    }
}

/// Staleness threshold in seconds (exposed for tests).
pub const fn stale_threshold_seconds() -> u64 {
    STALE_THRESHOLD_SECONDS
}

/// Maximum retries allowed per goal (exposed for tests).
pub const fn max_retries_per_goal() -> u32 {
    MAX_RETRIES_PER_GOAL
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(name: &str, goal: &str, depth: u32) -> SubordinateConfig {
        SubordinateConfig {
            agent_name: name.to_string(),
            goal: goal.to_string(),
            role: AgentRole::Engineer,
            worktree_path: PathBuf::from("/fake/worktree"),
            current_depth: depth,
        }
    }

    fn make_handle(name: &str) -> SubordinateHandle {
        SubordinateHandle {
            pid: 42,
            agent_name: name.to_string(),
            goal: "test goal".to_string(),
            worktree_path: PathBuf::from("/fake"),
            spawn_time: 1000,
            retry_count: 0,
            killed: false,
        }
    }

    // -- SubordinateConfig::validate --

    #[test]
    fn validate_accepts_valid_config() {
        let config = make_config("agent-1", "build feature", 0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_agent_name() {
        let config = make_config("", "build feature", 0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_goal() {
        let config = make_config("agent-1", "", 0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_depth_at_limit() {
        let limit = max_subordinate_depth();
        let config = make_config("agent-1", "goal", limit);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("depth"), "{err}");
    }

    #[test]
    fn validate_accepts_depth_below_limit() {
        let config = make_config("agent-1", "goal", 0);
        assert!(config.validate().is_ok());
    }

    // -- SubordinateHandle --

    #[test]
    fn can_retry_returns_true_when_below_limit() {
        let handle = make_handle("agent-1");
        assert!(handle.can_retry());
    }

    #[test]
    fn can_retry_returns_false_at_limit() {
        let mut handle = make_handle("agent-1");
        handle.retry_count = MAX_RETRIES_PER_GOAL;
        assert!(!handle.can_retry());
    }

    #[test]
    fn record_retry_increments_and_returns_count() {
        let mut handle = make_handle("agent-1");
        assert_eq!(handle.record_retry(), 1);
        assert_eq!(handle.record_retry(), 2);
        assert_eq!(handle.retry_count, 2);
    }

    #[test]
    fn handle_display_contains_key_fields() {
        let handle = make_handle("test-agent");
        let display = format!("{handle}");
        assert!(display.contains("test-agent"), "{display}");
        assert!(display.contains("pid=42"), "{display}");
    }

    // -- HeartbeatStatus --

    #[test]
    fn heartbeat_alive_display() {
        let status = HeartbeatStatus::Alive {
            last_epoch: 999,
            phase: "observing".to_string(),
        };
        let s = format!("{status}");
        assert!(s.contains("alive"));
        assert!(s.contains("999"));
        assert!(s.contains("observing"));
    }

    #[test]
    fn heartbeat_stale_display() {
        let status = HeartbeatStatus::Stale { seconds_since: 300 };
        let s = format!("{status}");
        assert!(s.contains("stale"));
        assert!(s.contains("300"));
    }

    #[test]
    fn heartbeat_dead_display() {
        let status = HeartbeatStatus::Dead;
        assert_eq!(format!("{status}"), "dead");
    }

    // -- constants --

    #[test]
    fn stale_threshold_is_positive() {
        assert!(stale_threshold_seconds() > 0);
    }

    #[test]
    fn max_retries_is_positive() {
        assert!(max_retries_per_goal() > 0);
    }
}
