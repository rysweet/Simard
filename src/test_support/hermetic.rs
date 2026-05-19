//! Hermetic test-state helper for cognitive-memory writers.
//!
//! Constructs a `TempDir`-backed state root, sets `SIMARD_STATE_ROOT` to
//! it for the helper's lifetime, and unsets `SIMARD_MEMORY_SOCKET` so the
//! socket path follows the state root automatically. The `Drop` impl
//! restores the previous env-var values, so two `HermeticState` instances
//! in the same test (or in nested calls) do not cross-contaminate.
//!
//! See `docs/testing/hermetic-tests.md` for the full contract and the
//! migration recipe for existing tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::memory_ipc::MEMORY_SOCKET_ENV;
use crate::state_root::STATE_ROOT_ENV;

/// RAII helper that allocates a hermetic state root and pins the relevant
/// env vars for its lifetime.
///
/// Construct one at the top of every test that touches cognitive memory:
///
/// ```ignore
/// use simard::test_support::HermeticState;
///
/// #[test]
/// #[serial_test::serial(cognitive_memory)]
/// fn my_persistence_test() {
///     let state = HermeticState::new();
///     // SIMARD_STATE_ROOT == state.state_root()
///     // SIMARD_MEMORY_SOCKET is unset → socket_path_for(state_root)
///     //   resolves to <state_root>/memory.sock
///     let bridge = launch_writer_bridge(state.state_root()).expect("bridge");
///     save_goal_board(&board, bridge.ops()).expect("save");
/// }
/// ```
pub struct HermeticState {
    // Field order matters: env-var bindings are restored on Drop BEFORE
    // the TempDir is reaped, so callers that hold a writer bridge still
    // see SIMARD_STATE_ROOT == temp path while their writer drains.
    _state_root_guard: EnvBinding,
    _socket_guard: EnvBinding,
    state_root: PathBuf,
    _temp: TempDir,
}

impl HermeticState {
    /// Allocate a fresh hermetic state root inside `env::temp_dir()`,
    /// set `SIMARD_STATE_ROOT` to it, and unset `SIMARD_MEMORY_SOCKET`.
    /// The temp dir, env-var bindings, and any registered in-process
    /// writer are torn down on `Drop`.
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("HermeticState: tempfile::tempdir failed");
        Self::new_with_temp(temp)
    }

    /// Allocate the hermetic state root under `parent` rather than
    /// `env::temp_dir()`. Used by tests whose `$TMPDIR` is mis-configured
    /// (e.g. `~/tmp`) and would otherwise trip the (H2) HOME guard.
    pub fn new_in(parent: &Path) -> Self {
        let temp = tempfile::tempdir_in(parent)
            .expect("HermeticState: tempfile::tempdir_in failed under parent");
        Self::new_with_temp(temp)
    }

    fn new_with_temp(temp: TempDir) -> Self {
        let state_root = temp.path().to_path_buf();
        // The temp dir is already a writable directory; nothing else to
        // create. Pin env vars LAST so a panic between create+pin still
        // leaves the env in its prior state.
        let state_root_guard = EnvBinding::set(STATE_ROOT_ENV, state_root.as_os_str());
        let socket_guard = EnvBinding::unset(MEMORY_SOCKET_ENV);
        Self {
            _state_root_guard: state_root_guard,
            _socket_guard: socket_guard,
            state_root,
            _temp: temp,
        }
    }

    /// Path of the hermetic state root. Caller passes this into
    /// `launch_writer_bridge` / `open_reader_bridge` etc.
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Resolved socket path under the hermetic state root —
    /// `<state_root>/memory.sock` when `SIMARD_MEMORY_SOCKET` is unset
    /// (which `new()` guarantees inside its lifetime).
    pub fn socket_path(&self) -> PathBuf {
        crate::memory_ipc::socket_path_for(&self.state_root)
    }
}

impl Default for HermeticState {
    fn default() -> Self {
        Self::new()
    }
}

/// Local RAII env-binding helper. Identical contract to the one tests
/// use directly, but routed through this module so production tests can
/// drop their per-file `EnvGuard` copies and import this one instead.
struct EnvBinding {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvBinding {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests using HermeticState are serialised via
        // `#[serial(cognitive_memory)]`, so concurrent env mutation is
        // excluded by the harness.
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

impl Drop for EnvBinding {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
