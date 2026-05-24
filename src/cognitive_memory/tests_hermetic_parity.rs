//! Hermetic guard parity tests (issue #1976).
//!
//! Verifies that every mutating `CognitiveMemoryOps` method on
//! `NativeCognitiveMemory` trips the hermetic-state-root guard when
//! `self.path` is under `$HOME/.simard`. One `#[should_panic]` test per
//! method, plus a corresponding positive test proving the guard does not
//! false-positive for legitimately hermetic (TempDir-rooted) instances.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use serial_test::serial;
use tempfile::TempDir;

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::memory_ipc::TEST_ALLOW_LIVE_STATE_ENV;
use crate::state_root::STATE_ROOT_ENV;

// ─── env helpers ───────────────────────────────────────────────────────────

struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev }
    }
    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Create a fake `$HOME` under a temp dir so that a
/// `NativeCognitiveMemory::open` rooted at `<home>/.simard/state` trips
/// the hermetic guard.
fn fake_home_under_temp() -> (TempDir, PathBuf, PathBuf) {
    let home_tmp = tempfile::tempdir().expect("home tempdir");
    let home = home_tmp.path().to_path_buf();
    let state_root = home.join(".simard").join("state");
    std::fs::create_dir_all(&state_root).expect("create fake state root");
    (home_tmp, home, state_root)
}

/// Open a `NativeCognitiveMemory` at the given `state_root`.
fn open_mem(state_root: &std::path::Path) -> NativeCognitiveMemory {
    NativeCognitiveMemory::open(state_root).expect("open DB at fake home state root")
}

// ═══════════════════════════════════════════════════════════════════════════
// Negative path: each mutating method panics when path is under HOME/.simard
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn record_sensory_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.record_sensory("test", "data", 60);
    }));
    assert!(
        panicked.is_err(),
        "record_sensory must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn prune_expired_sensory_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.prune_expired_sensory();
    }));
    assert!(
        panicked.is_err(),
        "prune_expired_sensory must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn push_working_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.push_working("slot", "content", "task-1", 0.5);
    }));
    assert!(
        panicked.is_err(),
        "push_working must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn clear_working_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.clear_working("task-1");
    }));
    assert!(
        panicked.is_err(),
        "clear_working must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn store_episode_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.store_episode("content", "test-source", None);
    }));
    assert!(
        panicked.is_err(),
        "store_episode must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn consolidate_episodes_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.consolidate_episodes(10);
    }));
    assert!(
        panicked.is_err(),
        "consolidate_episodes must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn store_procedure_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.store_procedure("proc", &["step1".into()], &[]);
    }));
    assert!(
        panicked.is_err(),
        "store_procedure must trip the hermetic guard"
    );
}

#[test]
#[serial(cognitive_memory)]
fn store_prospective_trips_guard_under_home_simard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = open_mem(&state_root);
        let _ = mem.store_prospective("desc", "trigger", "action", 1);
    }));
    assert!(
        panicked.is_err(),
        "store_prospective must trip the hermetic guard"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Positive path: TempDir-rooted memory does NOT trip the guard
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn hermetic_memory_does_not_trip_guard_for_any_mutating_method() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _state = EnvGuard::set(STATE_ROOT_ENV, tmp.path().to_str().unwrap());
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let mem = NativeCognitiveMemory::open(tmp.path()).expect("open DB at hermetic TempDir");

    // All nine mutating methods must succeed without panic.
    mem.record_sensory("test", "data", 60)
        .expect("record_sensory");
    mem.prune_expired_sensory().expect("prune_expired_sensory");
    mem.push_working("slot", "content", "task-1", 0.5)
        .expect("push_working");
    mem.clear_working("task-1").expect("clear_working");
    mem.store_episode("episode-content", "test-source", None)
        .expect("store_episode");
    mem.consolidate_episodes(10).expect("consolidate_episodes");
    mem.store_fact("concept", "content", 1.0, &["tag".into()], "src")
        .expect("store_fact");
    mem.store_procedure("proc", &["step1".into()], &[])
        .expect("store_procedure");
    mem.store_prospective("desc", "trigger", "action", 1)
        .expect("store_prospective");
}
