use std::path::PathBuf;

use crate::agent_goal_assignment::SubordinateProgress;
use crate::agent_roles::AgentRole;
use crate::error::SimardError;
use crate::identity_composition::max_subordinate_depth;

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
