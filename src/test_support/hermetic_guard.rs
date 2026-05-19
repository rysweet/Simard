//! `cfg(test)`-only hermetic state-root guard.
//!
//! Each guarded production write site (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) calls
//! [`assert_state_root_isolated`] before touching cognitive memory.
//! Tripping the guard panics with a message that names the offending
//! path and points operators at `docs/testing/hermetic-tests.md`.
//!
//! The guard is **compiled out of release builds** via `cfg(test)`; it
//! is a regression safety net for the cargo-test harness, not a
//! production check. Use `SIMARD_TEST_ALLOW_LIVE_STATE=1` to opt out
//! (only the install harness should do this).
//!
//! The check is conservative: it only fails when `state_root` is under
//! `$HOME/.simard`. A TempDir state root that happens to live outside
//! `$HOME` passes silently. The (H2) "not under HOME/.simard" property
//! is what distinguishes a hermetic test from one writing into the
//! operator's live cognitive memory.

use std::path::{Path, PathBuf};

use crate::memory_ipc::TEST_ALLOW_LIVE_STATE_ENV;

/// Assert that `state_root` is hermetic — i.e. **not** under
/// `$HOME/.simard`. `call_site` is logged in the panic message so the
/// failed test points directly at the offending production site.
pub fn assert_state_root_isolated(state_root: &Path, call_site: &'static str) {
    if std::env::var_os(TEST_ALLOW_LIVE_STATE_ENV).as_deref() == Some(std::ffi::OsStr::new("1")) {
        return;
    }

    let home_simard = match home_simard_path() {
        Some(p) => p,
        None => return,
    };

    let canon_state = state_root
        .canonicalize()
        .unwrap_or_else(|_| state_root.to_path_buf());
    let canon_home = home_simard
        .canonicalize()
        .unwrap_or_else(|_| home_simard.clone());

    if canon_state.starts_with(&canon_home) || state_root.starts_with(&home_simard) {
        panic!(
            "hermetic-test-state guard tripped at {call_site}: state_root {} is under \
             $HOME/.simard ({}); cognitive-memory writes from cargo-test must use a \
             TempDir state root (use `crate::test_support::HermeticState`). See \
             docs/testing/hermetic-tests.md. To opt out for the install harness, set \
             SIMARD_TEST_ALLOW_LIVE_STATE=1.",
            state_root.display(),
            home_simard.display(),
        );
    }
}

fn home_simard_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home);
    if p.as_os_str().is_empty() {
        return None;
    }
    Some(p.join(".simard"))
}
