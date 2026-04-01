//! Subordinate lifecycle management for agent composition.
//!
//! The supervisor spawns subordinate agents as child processes, tracks their
//! liveness via heartbeats stored in the hive (cognitive memory), and can
//! kill them when they become unresponsive or complete their goals.
//!
//! Each subordinate gets its own `agent_name` for memory isolation per
//! Pillar 8 (Identity != Runtime). Communication is exclusively through
//! semantic facts in the hive, never raw IPC.

use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_goal_assignment::{SubordinateProgress, poll_progress};
use crate::agent_roles::AgentRole;
use crate::error::{SimardError, SimardResult};
use crate::identity_composition::max_subordinate_depth;
use crate::memory_bridge::CognitiveMemoryBridge;

/// Maximum retries per goal before the supervisor gives up.
const MAX_RETRIES_PER_GOAL: u32 = 2;

/// Seconds after which a subordinate is considered stale.
const STALE_THRESHOLD_SECONDS: u64 = 120;

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

/// Spawn a subordinate agent as a real child process.
///
/// Forks a new Simard process via `Command::new(current_exe())` in the
/// given worktree, passing `--agent-name`, `--goal`, and `--depth` as
/// arguments. The child process inherits the parent's environment.
///
/// The function validates the configuration (depth limits, non-empty
/// fields) before spawning.
pub fn spawn_subordinate(config: &SubordinateConfig) -> SimardResult<SubordinateHandle> {
    config.validate()?;

    let now = current_epoch_seconds()?;

    let exe = std::env::current_exe().map_err(|e| SimardError::BridgeSpawnFailed {
        bridge: "subordinate".to_string(),
        reason: format!("cannot resolve current executable: {e}"),
    })?;

    let child = Command::new(&exe)
        .arg("engineer")
        .arg("run")
        .arg("local")
        .arg(&config.worktree_path)
        .arg(&config.goal)
        .env("SIMARD_AGENT_NAME", &config.agent_name)
        .env(
            "SIMARD_SUBORDINATE_DEPTH",
            (config.current_depth + 1).to_string(),
        )
        .current_dir(&config.worktree_path)
        .spawn()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "failed to spawn subordinate '{}' at '{}': {e}",
                config.agent_name,
                exe.display()
            ),
        })?;

    let pid = child.id();

    Ok(SubordinateHandle {
        pid,
        agent_name: config.agent_name.clone(),
        goal: config.goal.clone(),
        worktree_path: config.worktree_path.clone(),
        spawn_time: now,
        retry_count: 0,
        killed: false,
    })
}

/// Check the heartbeat of a subordinate by polling progress from the hive.
///
/// Returns `HeartbeatStatus::Alive` if a recent progress report exists,
/// `Stale` if the last report is older than the threshold, or `Dead` if
/// no progress has ever been reported.
pub fn check_heartbeat(
    handle: &SubordinateHandle,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<HeartbeatStatus> {
    if handle.killed {
        return Ok(HeartbeatStatus::Dead);
    }

    let progress = poll_progress(&handle.agent_name, bridge)?;

    match progress {
        None => Ok(HeartbeatStatus::Dead),
        Some(progress) => {
            let now = current_epoch_seconds()?;
            let elapsed = now.saturating_sub(progress.heartbeat_epoch);

            if elapsed > STALE_THRESHOLD_SECONDS {
                Ok(HeartbeatStatus::Stale {
                    seconds_since: elapsed,
                })
            } else {
                Ok(HeartbeatStatus::Alive {
                    last_epoch: progress.heartbeat_epoch,
                    phase: progress.phase,
                })
            }
        }
    }
}

/// Kill a subordinate by sending SIGTERM (Unix) or terminating the process.
///
/// Marks the handle as killed and sends a termination signal to the real
/// child process. On Unix, this sends SIGTERM via `kill(2)`. The handle
/// is mutated in place so the supervisor can track that it was explicitly
/// terminated.
pub fn kill_subordinate(handle: &mut SubordinateHandle) -> SimardResult<()> {
    if handle.killed {
        return Err(SimardError::InvalidIdentityComposition {
            identity: handle.agent_name.clone(),
            reason: "subordinate is already killed".to_string(),
        });
    }

    // Send SIGTERM to the real child process (pid > 0).
    if handle.pid > 0 {
        #[cfg(unix)]
        {
            // SAFETY: kill(2) is safe to call with a valid PID and signal.
            let ret = unsafe { libc::kill(handle.pid as libc::pid_t, libc::SIGTERM) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                // ESRCH means the process already exited — that's fine.
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(SimardError::ActionExecutionFailed {
                        action: format!("kill subordinate '{}'", handle.agent_name),
                        reason: format!("SIGTERM to pid {} failed: {err}", handle.pid),
                    });
                }
            }
        }
    }

    handle.killed = true;
    Ok(())
}

/// Determine whether a subordinate's progress indicates completion.
pub fn is_goal_complete(progress: &SubordinateProgress) -> bool {
    progress.outcome.is_some()
}

/// Get the current unix epoch in seconds.
fn current_epoch_seconds() -> SimardResult<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        }
    })?;
    Ok(duration.as_secs())
}

/// Maximum retries allowed per goal (exposed for tests).
pub const fn max_retries_per_goal() -> u32 {
    MAX_RETRIES_PER_GOAL
}

/// Staleness threshold in seconds (exposed for tests).
pub const fn stale_threshold_seconds() -> u64 {
    STALE_THRESHOLD_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SubordinateConfig {
        SubordinateConfig {
            agent_name: "sub-engineer-1".to_string(),
            goal: "implement the parser".to_string(),
            role: AgentRole::Engineer,
            worktree_path: PathBuf::from("/tmp/test-worktree"),
            current_depth: 0,
        }
    }

    /// Create a test handle without spawning a real process.
    fn test_handle() -> SubordinateHandle {
        SubordinateHandle {
            pid: 0,
            agent_name: "sub-engineer-1".to_string(),
            goal: "implement the parser".to_string(),
            worktree_path: PathBuf::from("/tmp/test-worktree"),
            spawn_time: 1_700_000_000,
            retry_count: 0,
            killed: false,
        }
    }

    #[test]
    fn spawn_rejects_empty_agent_name() {
        let mut config = test_config();
        config.agent_name = String::new();
        let err = spawn_subordinate(&config).expect_err("empty name should fail");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn spawn_rejects_empty_goal() {
        let mut config = test_config();
        config.goal = String::new();
        let err = spawn_subordinate(&config).expect_err("empty goal should fail");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn spawn_rejects_excessive_depth() {
        let mut config = test_config();
        config.current_depth = max_subordinate_depth();
        let err = spawn_subordinate(&config).expect_err("excessive depth should fail");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn handle_fields_are_correct() {
        let handle = test_handle();
        assert_eq!(handle.agent_name, "sub-engineer-1");
        assert_eq!(handle.goal, "implement the parser");
        assert_eq!(handle.retry_count, 0);
        assert!(!handle.killed);
    }

    #[test]
    fn kill_subordinate_marks_killed() {
        let mut handle = test_handle();
        assert!(!handle.killed);
        kill_subordinate(&mut handle).expect("kill should succeed");
        assert!(handle.killed);
    }

    #[test]
    fn kill_already_killed_subordinate_fails() {
        let mut handle = test_handle();
        kill_subordinate(&mut handle).expect("first kill should succeed");
        let err = kill_subordinate(&mut handle).expect_err("second kill should fail");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn retry_tracking_works() {
        let mut handle = test_handle();
        assert!(handle.can_retry());
        assert_eq!(handle.record_retry(), 1);
        assert!(handle.can_retry());
        assert_eq!(handle.record_retry(), 2);
        assert!(!handle.can_retry());
    }

    #[test]
    fn handle_display_is_readable() {
        let handle = test_handle();
        let display = handle.to_string();
        assert!(display.contains("sub-engineer-1"));
        assert!(display.contains("retries=0"));
    }

    #[test]
    fn heartbeat_status_display_covers_all_variants() {
        let alive = HeartbeatStatus::Alive {
            last_epoch: 100,
            phase: "execution".to_string(),
        };
        assert!(alive.to_string().contains("alive"));

        let stale = HeartbeatStatus::Stale { seconds_since: 300 };
        assert!(stale.to_string().contains("stale"));

        let dead = HeartbeatStatus::Dead;
        assert_eq!(dead.to_string(), "dead");
    }

    #[test]
    fn is_goal_complete_checks_outcome() {
        let p = SubordinateProgress {
            sub_id: "test".to_string(),
            phase: "complete".to_string(),
            steps_completed: 10,
            steps_total: 10,
            last_action: "done".to_string(),
            heartbeat_epoch: 12345,
            outcome: None,
        };
        assert!(!is_goal_complete(&p));

        let p2 = p.with_outcome("success");
        assert!(is_goal_complete(&p2));
    }
}
