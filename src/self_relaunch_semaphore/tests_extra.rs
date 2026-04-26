use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use semaphore::{epoch_now, extract_u64, is_pid_alive};

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
fn handoff_config_defaults() {
    use crate::self_relaunch::RelaunchConfig;
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
    assert_eq!(cfg.gates.len(), 4);
    assert_eq!(cfg.child_ready_timeout, Duration::from_secs(45));

    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn coordinated_handoff_rejects_non_leader() {
    use crate::self_relaunch::RelaunchConfig;
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let my_pid = std::process::id();
    sem.try_acquire(my_pid).unwrap();

    let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
    let err = coordinated_handoff(99999, &cfg).unwrap_err();
    assert!(err.to_string().contains("not current leader"));

    let _ = fs::remove_file(&lock);
    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn coordinated_handoff_rejects_no_state() {
    use crate::self_relaunch::RelaunchConfig;
    let lock = temp_lock_path();
    let sem = LeaderSemaphore::new(&lock);
    let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
    let err = coordinated_handoff(1234, &cfg).unwrap_err();
    assert!(err.to_string().contains("no leader state"));

    let _ = fs::remove_dir(lock.parent().unwrap());
}

#[test]
fn is_pid_alive_self() {
    assert!(is_pid_alive(std::process::id()));
}

#[test]
fn is_pid_alive_dead() {
    // PID 99999999 is almost certainly not alive.
    assert!(!is_pid_alive(99999999));
}

// Tests previously inlined in semaphore.rs (#1266 burndown)
mod semaphore_inline {
    use crate::self_relaunch_semaphore::semaphore::*;
    use std::path::PathBuf;

    // ── LeaderState JSON round-trip ─────────────────────────────────

    #[test]
    fn leader_state_json_round_trip() {
        let state = LeaderState {
            pid: 42,
            generation: 7,
            heartbeat_epoch: 1_700_000_000,
        };
        let json = state.to_json();
        let back = LeaderState::from_json(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn leader_state_from_json_missing_field_returns_none() {
        assert!(LeaderState::from_json(r#"{"pid":1,"generation":2}"#).is_none());
        assert!(LeaderState::from_json("").is_none());
        assert!(LeaderState::from_json("not json").is_none());
    }

    // ── extract_u64 ─────────────────────────────────────────────────

    #[test]
    fn extract_u64_works() {
        let json = r#"{"pid":123,"generation":456}"#;
        assert_eq!(extract_u64(json, "pid"), Some(123));
        assert_eq!(extract_u64(json, "generation"), Some(456));
        assert_eq!(extract_u64(json, "missing"), None);
    }

    // ── LeaderSemaphore acquire / release / transfer ────────────────

    #[test]
    fn acquire_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("leader.json");
        let sem = LeaderSemaphore::new(&lock);

        let state = sem.try_acquire(100).unwrap();
        assert_eq!(state.pid, 100);
        assert_eq!(state.generation, 1);

        let read = sem.read_state().unwrap().unwrap();
        assert_eq!(read, state);
    }

    #[test]
    fn acquire_twice_same_pid_refreshes_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json"));

        let first = sem.try_acquire(100).unwrap();
        let second = sem.try_acquire(100).unwrap();
        assert_eq!(second.generation, first.generation);
        assert!(second.heartbeat_epoch >= first.heartbeat_epoch);
    }

    #[test]
    fn acquire_fails_when_another_pid_alive() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json"));

        // Use our own PID — guaranteed alive and accessible.
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();
        let result = sem.try_acquire(my_pid + 99999);
        assert!(result.is_err());
    }

    #[test]
    fn acquire_seizes_from_stale_leader() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json")).with_stale_threshold(0);

        // Write a state with PID 1 (alive but will be stale due to threshold=0)
        sem.try_acquire(1).unwrap();
        // Sleep briefly to ensure staleness
        std::thread::sleep(std::time::Duration::from_millis(10));
        let seized = sem.try_acquire(200).unwrap();
        assert_eq!(seized.pid, 200);
        assert_eq!(seized.generation, 2);
    }

    #[test]
    fn release_removes_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("leader.json");
        let sem = LeaderSemaphore::new(&lock);

        sem.try_acquire(100).unwrap();
        assert!(lock.exists());

        sem.release(100).unwrap();
        assert!(!lock.exists());
    }

    #[test]
    fn release_noop_for_non_owner() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("leader.json");
        let sem = LeaderSemaphore::new(&lock);

        sem.try_acquire(100).unwrap();
        sem.release(999).unwrap(); // not the owner
        assert!(lock.exists()); // file still there
    }

    #[test]
    fn transfer_increments_generation() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json"));

        let orig = sem.try_acquire(100).unwrap();
        let transferred = sem.transfer(100, 200).unwrap();
        assert_eq!(transferred.pid, 200);
        assert_eq!(transferred.generation, orig.generation + 1);
    }

    #[test]
    fn transfer_fails_for_non_owner() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json"));

        sem.try_acquire(100).unwrap();
        let result = sem.transfer(999, 200);
        assert!(result.is_err());
    }

    #[test]
    fn heartbeat_updates_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("leader.json"));

        let orig = sem.try_acquire(100).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        sem.heartbeat(100).unwrap();

        let updated = sem.read_state().unwrap().unwrap();
        assert!(updated.heartbeat_epoch >= orig.heartbeat_epoch);
    }

    #[test]
    fn read_state_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let sem = LeaderSemaphore::new(dir.path().join("no-such.json"));
        assert!(sem.read_state().unwrap().is_none());
    }

    #[test]
    fn lock_path_accessor() {
        let path = PathBuf::from("/tmp/test-lock.json");
        let sem = LeaderSemaphore::new(&path);
        assert_eq!(sem.lock_path(), path);
    }
}
