//! Issue #1967 regression pin — `memory_ipc::default_state_root` must
//! agree with `simard::state_root::simard_state_root`.
//!
//! Before this fix, `memory_ipc::default_state_root` returned
//! `$HOME/.simard/state` while the daemon, `simard goal` CLI, and
//! `simard::state_root::simard_state_root()` returned `$HOME/.simard`.
//! The result: the meeting REPL's direct-open fallback opened a
//! different LadybugDB than the one the daemon owned, so meetings could
//! not see or modify the real goal board.

use super::default_state_root;
use crate::state_root::simard_state_root;

/// The single most important invariant for issue #1967: the two
/// resolvers must return identical paths in every environment.
#[test]
fn default_state_root_matches_canonical_simard_state_root() {
    let from_memory_ipc = default_state_root();
    let from_canonical = simard_state_root();
    assert_eq!(
        from_memory_ipc, from_canonical,
        "memory_ipc::default_state_root must equal simard::state_root::simard_state_root \
         — issue #1967 regressed when these diverged"
    );
}

/// When `SIMARD_STATE_ROOT` is set, both resolvers must honour it
/// identically (no implicit subdirectory join on either side).
#[test]
fn explicit_state_root_env_is_honoured_exactly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_path_buf();
    let prev = std::env::var_os("SIMARD_STATE_ROOT");
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &path);
    }

    let from_memory_ipc = default_state_root();
    let from_canonical = simard_state_root();

    // Restore env before any assertion so a failure does not leak.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
            None => std::env::remove_var("SIMARD_STATE_ROOT"),
        }
    }

    assert_eq!(
        from_memory_ipc, path,
        "memory_ipc must honour SIMARD_STATE_ROOT exactly, no subdir join"
    );
    assert_eq!(
        from_canonical, path,
        "canonical resolver must honour SIMARD_STATE_ROOT exactly"
    );
    assert_eq!(from_memory_ipc, from_canonical);
}
