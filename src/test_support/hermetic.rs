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
///     let memory = launch_writer_client(state.state_root()).expect("memory");
///     save_goal_board(&board, memory.ops()).expect("save");
/// }
/// ```
pub struct HermeticState {
    // Field order matters: env-var bindings are restored on Drop BEFORE
    // the TempDir is reaped, so callers that hold a writer memory still
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
    /// `launch_writer_client` / `open_reader_client` etc.
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

/// Internal RAII env save/restore used by [`HermeticState`]: it records a
/// variable's prior value, sets/unsets it, and restores it on `Drop`. It is
/// module-private and intentionally NOT re-exported — among the env helpers,
/// `HermeticState` is the one `test_support` exposes; tests must not reach for
/// a shared env guard. The migration that closed issue #2360 is annotation-only
/// (add the serial key), not a body rewrite to import a guard.
struct EnvBinding {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvBinding {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let prev = std::env::var_os(key);
        // INVARIANT (issue #2360): EVERY test in the lib binary that touches
        // cognitive memory OR mutates/reads process-global env (SIMARD_STATE_ROOT
        // set + SIMARD_MEMORY_SOCKET unset here; HOME and any other var
        // elsewhere) MUST be keyed into the `serial(cognitive_memory)` group.
        // HermeticState mutates process-global env, and glibc setenv/getenv are
        // not thread-safe, so a concurrent env mutation in any other test can
        // tear a handler's `std::env::var("SIMARD_STATE_ROOT")` read and send
        // writes to HOME/.simard — the race behind the tests_goals_crud flake.
        // The `serial_guard` meta-test (src/test_support/serial_guard.rs)
        // auto-enforces this for its watched surface (SIMARD_STATE_ROOT /
        // SIMARD_MEMORY_SOCKET / HOME / SIMARD_LLM_PROVIDER / SIMARD_MEETINGS_DIR
        // / SIMARD_MEETINGS_ROOT); keying any OTHER var is an author obligation
        // the guard does not yet check (EnvWatch::AnyVar tracked as #2375). See
        // docs/testing/cognitive-memory-serial-isolation.md.
        //
        // SAFETY: tests using HermeticState are serialised via
        // `#[serial(cognitive_memory)]`, so concurrent env mutation is
        // excluded by the harness — the invariant above is what makes this
        // `set_var` sound.
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
