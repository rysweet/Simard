use super::*;
use std::time::Duration;

fn test_lock() -> (tempfile::TempDir, BuildLock) {
    let dir = tempfile::TempDir::new().unwrap();
    let bl = BuildLock::new(dir.path());
    (dir, bl)
}

#[test]
fn try_acquire_succeeds_when_unlocked() {
    let (_dir, bl) = test_lock();
    let guard = bl.try_acquire().unwrap();
    assert!(guard.is_some());
    assert!(bl.is_locked());
}

#[test]
fn try_acquire_returns_none_when_locked() {
    let (_dir, bl) = test_lock();
    let _guard = bl.try_acquire().unwrap().unwrap();
    let second = bl.try_acquire().unwrap();
    assert!(second.is_none());
}

#[test]
fn drop_guard_releases_lock() {
    let (_dir, bl) = test_lock();
    {
        let _guard = bl.try_acquire().unwrap().unwrap();
        assert!(bl.is_locked());
    }
    assert!(!bl.is_locked());
}

#[test]
fn acquire_with_timeout_succeeds_when_free() {
    let (_dir, bl) = test_lock();
    let _guard = bl.acquire(Duration::from_secs(1)).unwrap();
    assert!(bl.is_locked());
}

#[test]
fn acquire_with_timeout_fails_when_locked() {
    let (_dir, bl) = test_lock();
    let _guard = bl.try_acquire().unwrap().unwrap();
    let result = bl.acquire(Duration::from_millis(100));
    assert!(result.is_err());
}

#[test]
fn force_release_clears_lock() {
    let (_dir, bl) = test_lock();
    let _guard = bl.try_acquire().unwrap().unwrap();
    // manually leak the guard so we can test force_release
    std::mem::forget(_guard);

    assert!(bl.is_locked());
    assert!(bl.force_release().unwrap());
    assert!(!bl.is_locked());
}

#[test]
fn force_release_when_not_locked() {
    let (_dir, bl) = test_lock();
    assert!(!bl.force_release().unwrap());
}

#[test]
fn current_holder_returns_info() {
    let (_dir, bl) = test_lock();
    assert!(bl.current_holder().is_none());
    let _guard = bl.try_acquire().unwrap().unwrap();
    let info = bl.current_holder().unwrap();
    assert!(info.contains("pid="));
}

#[test]
fn stale_lock_from_dead_pid_is_reaped() {
    let (dir, _bl) = test_lock();
    // Manually write a lock with a dead PID
    let lock_path = dir.path().join("cargo_build.lock");
    std::fs::write(
        &lock_path,
        "pid=999999999\nhost=test\nstarted=2024-01-01T00:00:00Z\n",
    )
    .unwrap();

    let bl = BuildLock::new(dir.path());
    let guard = bl.try_acquire().unwrap();
    assert!(guard.is_some(), "Should reap dead PID lock and acquire");
}
