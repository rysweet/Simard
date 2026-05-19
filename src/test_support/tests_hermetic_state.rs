//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for the
//! [`super::HermeticState`] helper.
//!
//! Contract under test (see `docs/testing/hermetic-tests.md`):
//!
//! - (H1) `state_root()` returns a path under `env::temp_dir()`.
//! - (H2) `state_root()` is not equal to, and not a descendant of,
//!   `$HOME/.simard`.
//! - (H3) `socket_path()` is `<state_root>/memory.sock` when
//!   `SIMARD_MEMORY_SOCKET` is unset.
//! - (H4) `TempDir` outlives every bridge handle the test opens
//!   (covered by RAII drop order — the helper's destructor must reap
//!   the temp dir AFTER releasing the env vars, not before).
//! - Drop must restore the previous values of `SIMARD_STATE_ROOT` and
//!   `SIMARD_MEMORY_SOCKET`. Two helpers in sequence must not
//!   cross-contaminate.
//!
//! These tests fail until the implementation step replaces the
//! `unimplemented!` bodies in `src/test_support/hermetic.rs`.

use std::path::PathBuf;

use serial_test::serial;

use super::HermeticState;
use crate::memory_ipc::MEMORY_SOCKET_ENV;
use crate::state_root::STATE_ROOT_ENV;

/// Local env-guard so this test file does not depend on its own
/// subject-under-test.
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
    #[allow(dead_code)]
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

fn home_simard_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into());
    PathBuf::from(home).join(".simard")
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_state_root_lives_under_temp_dir() {
    let state = HermeticState::new();
    let root = state.state_root();

    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    assert!(
        canon.starts_with(&tmp),
        "H1: HermeticState::state_root() = {} must be under env::temp_dir() = {}",
        canon.display(),
        tmp.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_state_root_is_not_under_home_simard() {
    let state = HermeticState::new();
    let root = state.state_root();
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let home_simard = home_simard_path();
    assert!(
        !home_simard.as_os_str().is_empty() && !canon.starts_with(&home_simard),
        "H2: HermeticState::state_root() = {} must not be under HOME/.simard = {}",
        canon.display(),
        home_simard.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_socket_path_follows_state_root() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let socket = state.socket_path();

    assert_eq!(
        socket,
        root.join("memory.sock"),
        "H3: HermeticState::socket_path() must equal state_root.join(\"memory.sock\"); \
         got socket={} state_root={}",
        socket.display(),
        root.display(),
    );
    assert!(
        socket.starts_with(&root),
        "H3: socket path {} must be under the helper's own state root {}",
        socket.display(),
        root.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_state_sets_simard_state_root_env_var() {
    // Pin the env var to a sentinel before construction so we can
    // observe both the set-during-lifetime and restore-on-drop steps.
    let sentinel = "/sentinel/before-hermetic-state";
    let _guard = EnvGuard::set(STATE_ROOT_ENV, sentinel);

    {
        let state = HermeticState::new();
        let observed = std::env::var(STATE_ROOT_ENV).expect("env var must be set");
        let expected = state.state_root().to_string_lossy().to_string();
        assert_eq!(
            observed, expected,
            "during HermeticState lifetime, {} must equal state_root() = {}",
            STATE_ROOT_ENV, expected,
        );
    }

    // After drop, the previous sentinel must be restored.
    let after = std::env::var(STATE_ROOT_ENV).unwrap_or_default();
    assert_eq!(
        after, sentinel,
        "after Drop, {} must be restored to its pre-construction value ({}); got {}",
        STATE_ROOT_ENV, sentinel, after,
    );
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_state_unsets_simard_memory_socket_env_var() {
    // Even with a stray operator-set SIMARD_MEMORY_SOCKET in scope, the
    // helper must unset it for its lifetime so socket_path_for resolves
    // to the helper's own TempDir.
    let stray = "/some/stray/socket.sock";
    let _guard = EnvGuard::set(MEMORY_SOCKET_ENV, stray);

    {
        let _state = HermeticState::new();
        assert!(
            std::env::var_os(MEMORY_SOCKET_ENV).is_none(),
            "during HermeticState lifetime, {} must be unset; \
             a stray value defeats socket_path_for(state_root)",
            MEMORY_SOCKET_ENV,
        );
    }

    let after = std::env::var(MEMORY_SOCKET_ENV).unwrap_or_default();
    assert_eq!(
        after, stray,
        "after Drop, {} must be restored to its pre-construction stray \
         value ({}); got {}",
        MEMORY_SOCKET_ENV, stray, after,
    );
}

#[test]
#[serial(cognitive_memory)]
fn sequential_hermetic_states_do_not_cross_contaminate() {
    // Two helpers in sequence must each see their own state root.
    let root_a;
    {
        let a = HermeticState::new();
        root_a = a.state_root().to_path_buf();
        assert_eq!(
            std::env::var(STATE_ROOT_ENV).unwrap(),
            root_a.to_string_lossy()
        );
    }

    let root_b;
    {
        let b = HermeticState::new();
        root_b = b.state_root().to_path_buf();
        assert_eq!(
            std::env::var(STATE_ROOT_ENV).unwrap(),
            root_b.to_string_lossy()
        );
    }

    assert_ne!(
        root_a,
        root_b,
        "two sequential HermeticState helpers must allocate distinct \
         state roots; got the same path twice = {}",
        root_a.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn hermetic_state_root_is_a_writable_directory() {
    // Sanity: the helper must actually create the directory before
    // returning, so downstream `launch_writer_bridge(state_root)` calls
    // do not fail with ENOENT.
    let state = HermeticState::new();
    let root = state.state_root();

    let probe = root.join("hermetic-probe.txt");
    std::fs::write(&probe, b"writable").expect("hermetic state root must be a writable dir");
    let read = std::fs::read(&probe).expect("read-back");
    assert_eq!(read, b"writable");
}
