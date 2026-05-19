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
//!
//! # TDD stub
//!
//! The methods below are stubbed with `unimplemented!` for the
//! issue-#1923 / -#1925 TDD step. The real implementation lands in the
//! implementation step that follows.

use std::path::{Path, PathBuf};

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
    _marker: (),
}

impl HermeticState {
    /// Allocate a fresh hermetic state root inside `env::temp_dir()`,
    /// set `SIMARD_STATE_ROOT` to it, and unset `SIMARD_MEMORY_SOCKET`.
    /// The temp dir, env-var bindings, and any registered in-process
    /// writer are torn down on `Drop`.
    pub fn new() -> Self {
        unimplemented!(
            "HermeticState::new is the #1923/#1925 implementation surface — \
             stubbed for the TDD step; see docs/testing/hermetic-tests.md"
        )
    }

    /// Allocate the hermetic state root under `parent` rather than
    /// `env::temp_dir()`. Used by tests whose `$TMPDIR` is mis-configured
    /// (e.g. `~/tmp`) and would otherwise trip the (H2) HOME guard.
    pub fn new_in(_parent: &Path) -> Self {
        unimplemented!("HermeticState::new_in is the #1923/#1925 implementation surface")
    }

    /// Path of the hermetic state root. Caller passes this into
    /// `launch_writer_bridge` / `open_reader_bridge` etc.
    pub fn state_root(&self) -> &Path {
        unimplemented!("HermeticState::state_root — TDD stub")
    }

    /// Resolved socket path under the hermetic state root —
    /// `<state_root>/memory.sock` when `SIMARD_MEMORY_SOCKET` is unset
    /// (which `new()` guarantees inside its lifetime).
    pub fn socket_path(&self) -> PathBuf {
        unimplemented!("HermeticState::socket_path — TDD stub")
    }
}

impl Default for HermeticState {
    fn default() -> Self {
        Self::new()
    }
}
