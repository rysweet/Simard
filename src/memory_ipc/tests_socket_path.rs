//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for the
//! [`super::socket_path_for`] resolver.
//!
//! Contract under test (see `docs/reference/simard-cli.md#shared-socket-path-contract`
//! and `docs/testing/hermetic-tests.md`):
//!
//! 1. With no env override, `socket_path_for(state_root)` returns
//!    `<state_root>/memory.sock` — the socket follows the state root and
//!    `SIMARD_STATE_ROOT` becomes actually hermetic.
//! 2. With `SIMARD_MEMORY_SOCKET=/some/path`, the function returns that
//!    path verbatim, regardless of `state_root`.
//! 3. The legacy `default_socket_path()` continues to return a path under
//!    `~/.simard/` until call sites are migrated — but for any path
//!    obtained via `socket_path_for(tempdir)` the result is under
//!    `tempdir`, never under `~/.simard/`.
//!
//! These tests fail until the implementation step replaces the
//! `unimplemented!` body in `src/memory_ipc/mod.rs`.

use std::path::PathBuf;

use serial_test::serial;

use super::{MEMORY_SOCKET_ENV, socket_path_for};

/// RAII helper that pins an env-var value for the duration of a test
/// even when the test panics. Local to this test module so the test
/// itself does not depend on the not-yet-implemented `HermeticState`.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests are serialised via `#[serial(cognitive_memory)]`,
        // so concurrent env mutation is excluded by the harness.
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

#[test]
#[serial(cognitive_memory)]
fn socket_path_for_without_env_override_is_under_state_root() {
    // (H1)/(H3) hermeticity unlock: a TempDir state root must produce a
    // socket path under the same TempDir.
    let _unset = EnvGuard::unset(MEMORY_SOCKET_ENV);

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let socket = socket_path_for(root);

    assert!(
        socket.starts_with(root),
        "socket_path_for must return a path under the state root when \
         SIMARD_MEMORY_SOCKET is unset; got socket={} for state_root={}",
        socket.display(),
        root.display(),
    );
    assert_eq!(
        socket.file_name(),
        Some(std::ffi::OsStr::new("memory.sock")),
        "default socket file name must be 'memory.sock'; got {}",
        socket.display(),
    );
    assert_eq!(
        socket,
        root.join("memory.sock"),
        "socket_path_for(state_root) without override must equal \
         state_root.join(\"memory.sock\")"
    );
}

#[test]
#[serial(cognitive_memory)]
fn socket_path_for_with_env_override_returns_override_verbatim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let override_path = std::env::temp_dir().join("simard-1923-override.sock");
    let _set = EnvGuard::set(MEMORY_SOCKET_ENV, override_path.to_str().unwrap());

    let socket = socket_path_for(root);

    assert_eq!(
        socket,
        override_path,
        "SIMARD_MEMORY_SOCKET must take precedence over state_root \
         resolution; got socket={} expected={}",
        socket.display(),
        override_path.display(),
    );
    // The override deliberately is NOT under `root` — proves precedence.
    assert!(
        !socket.starts_with(root),
        "override path must not be under the test state root; \
         the precedence test is meaningless otherwise"
    );
}

#[test]
#[serial(cognitive_memory)]
fn socket_path_for_ignores_empty_env_override() {
    // An empty value of the env var must be treated as "unset" so a
    // shell that exports `SIMARD_MEMORY_SOCKET=` (common boilerplate)
    // does not break the state-root-follow rule.
    let _set = EnvGuard::set(MEMORY_SOCKET_ENV, "");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let socket = socket_path_for(root);

    assert_eq!(
        socket,
        root.join("memory.sock"),
        "empty SIMARD_MEMORY_SOCKET must be ignored and resolution must \
         fall back to <state_root>/memory.sock; got {}",
        socket.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn socket_path_for_distinct_roots_yields_distinct_sockets() {
    // The key hermeticity property: two TempDir state roots in the same
    // process must produce disjoint socket paths. This is the property
    // that closes #1923/#1925 — a test pointing at a TempDir cannot
    // accidentally collide with the live daemon's socket at
    // `~/.simard/memory.sock`.
    let _unset = EnvGuard::unset(MEMORY_SOCKET_ENV);

    let a = tempfile::tempdir().expect("tempdir a");
    let b = tempfile::tempdir().expect("tempdir b");

    let sa = socket_path_for(a.path());
    let sb = socket_path_for(b.path());

    assert_ne!(
        sa,
        sb,
        "distinct state roots must yield distinct socket paths; \
         got both = {}",
        sa.display(),
    );

    let home_simard = home_simard_path();
    assert!(
        !sa.starts_with(&home_simard) && !sb.starts_with(&home_simard),
        "TempDir-rooted sockets must never resolve under HOME/.simard \
         ({}); got a={} b={}",
        home_simard.display(),
        sa.display(),
        sb.display(),
    );
}

fn home_simard_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into());
    PathBuf::from(home).join(".simard")
}
