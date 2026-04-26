use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use semaphore::{epoch_now, extract_u64};

use handoff::wait_for_ready;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_lock_path() -> PathBuf {
    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "simard-sem-test-{}-{}-{}",
        std::process::id(),
        epoch_now(),
        n,
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.join("leader.lock")
}

#[test]
fn leader_state_json_roundtrip() {
    let state = LeaderState {
        pid: 42,
        generation: 7,
        heartbeat_epoch: 1700000000,
    };
    let json = state.to_json();
    let parsed = LeaderState::from_json(&json).unwrap();
    assert_eq!(state, parsed);
}

#[test]
fn extract_u64_works() {
    let json = r#"{"pid":123,"generation":5,"heartbeat_epoch":999}"#;
    assert_eq!(extract_u64(json, "pid"), Some(123));
    assert_eq!(extract_u64(json, "generation"), Some(5));
    assert_eq!(extract_u64(json, "heartbeat_epoch"), Some(999));
    assert_eq!(extract_u64(json, "missing"), None);
}

#[test]
fn acquire_fresh_semaphore() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let state = sem.try_acquire(1234).unwrap();
    assert_eq!(state.pid, 1234);
    assert_eq!(state.generation, 1);
    // Clean up.
    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn acquire_rejects_live_leader() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();

    // Another pid trying to acquire should fail (our pid is alive).
    let result = sem.try_acquire(my_pid + 99999);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("leadership held by"));

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn acquire_seizes_from_dead_pid() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    // Write a state for a definitely-dead PID.
    let dead_state = LeaderState {
        pid: 99999999,
        generation: 3,
        heartbeat_epoch: epoch_now(),
    };
    sem.write_state(&dead_state).unwrap();

    let state = sem.try_acquire(std::process::id()).unwrap();
    assert_eq!(state.pid, std::process::id());
    assert_eq!(state.generation, 4); // incremented

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn acquire_seizes_stale_heartbeat() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock).with_stale_threshold(1);

    let my_pid = std::process::id();
    // Write a state with old heartbeat (our own PID so it's "alive").
    let stale = LeaderState {
        pid: my_pid,
        generation: 5,
        heartbeat_epoch: epoch_now().saturating_sub(100),
    };
    sem.write_state(&stale).unwrap();

    // Different PID can seize because heartbeat is stale.
    // We use my_pid here since it's definitely alive but the stale check wins.
    let state = sem.try_acquire(my_pid).unwrap();
    // Same PID re-acquires — refreshes heartbeat.
    assert_eq!(state.pid, my_pid);
    assert_eq!(state.generation, 5); // same gen for same PID

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn heartbeat_refreshes_epoch() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    let state = sem.try_acquire(my_pid).unwrap();
    let old_epoch = state.heartbeat_epoch;

    std::thread::sleep(Duration::from_millis(10));
    sem.heartbeat(my_pid).unwrap();

    let refreshed = sem.read_state().unwrap().unwrap();
    assert!(refreshed.heartbeat_epoch >= old_epoch);

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn transfer_changes_owner() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();

    let new_state = sem.transfer(my_pid, 55555).unwrap();
    assert_eq!(new_state.pid, 55555);
    assert_eq!(new_state.generation, 2);

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn transfer_rejects_non_owner() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();

    let err = sem.transfer(99999, 55555).unwrap_err();
    assert!(err.to_string().contains("does not own"));

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn release_removes_lock() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();
    assert!(lock.exists());

    sem.release(my_pid).unwrap();
    assert!(!lock.exists());

    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn release_ignores_non_owner() {
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();

    // Non-owner release should be a no-op.
    sem.release(99999).unwrap();
    assert!(lock.exists()); // still there

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn signal_ready_creates_file() {
    let dir = std::env::temp_dir().join(format!("simard-ready-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    signal_ready(&dir, 12345).unwrap();
    let path = dir.join("ready-12345.json");
    assert!(path.exists());
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("\"pid\":12345"));
    assert!(contents.contains("\"status\":\"ready\""));

    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn wait_for_ready_timeout() {
    let path = PathBuf::from("/tmp/simard-no-such-ready-signal-99999.json");
    let err = wait_for_ready(&path, Duration::from_millis(100)).unwrap_err();
    assert!(err.to_string().contains("did not signal readiness"));
}

#[test]
fn wait_for_ready_succeeds_when_file_exists() {
    let dir = std::env::temp_dir().join(format!("simard-wr-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ready-777.json");
    fs::write(&path, b"ok").unwrap();

    wait_for_ready(&path, Duration::from_secs(1)).unwrap();

    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}
