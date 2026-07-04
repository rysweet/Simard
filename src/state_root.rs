//! Resolved Simard durable state root and its subdirectories.
//!
//! Single helper shared by `simard meeting`, `simard goal-curation`, and the
//! OODA daemon. Before this module existed, the meeting REPL hardcoded
//! `~/.simard/meetings/` and ignored `SIMARD_STATE_ROOT` (issue #1906), while
//! `goal_curation::operations` carried its own duplicate copy. All
//! state-root-aware callers should now route through here.
//!
//! See `docs/reference/state-root-resolution.md` for the public contract,
//! the per-subsystem env-var precedence ladder, and the validation rules.
//!
//! ## Precedence (per subsystem)
//!
//! 1. The subsystem's narrow env var (e.g. `SIMARD_HANDOFF_DIR`,
//!    `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`) when set + non-empty.
//! 2. `$SIMARD_STATE_ROOT/<subdir>` when `SIMARD_STATE_ROOT` is set + valid.
//! 3. `$HOME/.simard/<subdir>` (default).
//!
//! The validation rules on `SIMARD_STATE_ROOT` are intentionally lightweight:
//! empty / relative / NUL-bearing values are silently ignored (with a WARN
//! emitted at first use) so a malformed env var never crashes boot.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing::warn;

/// Environment variable that relocates the durable state root for the whole
/// CLI (meetings, handoffs, goal board, future cognitive backups).
pub const STATE_ROOT_ENV: &str = "SIMARD_STATE_ROOT";

/// Default state-root directory name under `$HOME` when no env override is
/// present. Lifted out of the function to keep the constant single-sourced.
pub const DEFAULT_STATE_ROOT_DIRNAME: &str = ".simard";

/// Resolve the durable state-root directory.
///
/// Returns the first valid match from the ladder in the module-level docs.
/// Never panics; never creates the directory. The first writer is responsible
/// for `create_dir_all` on the resolved subdirectory.
pub fn simard_state_root() -> PathBuf {
    if let Some(p) = sanitized_env_state_root() {
        return p;
    }
    home_default()
}

/// Resolve a named subdirectory under [`simard_state_root`].
///
/// `name` must be a static, caller-chosen subdirectory string
/// (`"meetings"`, `"meeting_handoffs"`, `"goals"`, …). The helper does no
/// validation on `name`; pass static strings only.
pub fn resolve_subdir(name: &str) -> PathBuf {
    simard_state_root().join(name)
}

/// Canonical path for the file-backed goal store.
///
/// Resolves to `<state_root>/state/goal_store.json`. All consumers
/// (bootstrap assembly, meeting close, OODA curate) should use this
/// single helper to avoid path inconsistencies.
pub fn goal_store_path() -> PathBuf {
    simard_state_root().join("state").join("goal_store.json")
}

/// Canonical path of the authoritative goal-board file (issue #1).
///
/// Resolves to `<state_root>/state/goal_board.json`. This is the single durable
/// source of truth for the OODA goal board (see [`crate::goal_board_store`]);
/// distinct from the legacy [`goal_store_path`] (`goal_store.json`) used by the
/// pre-memory goal-records migration.
pub fn goal_board_json_path() -> PathBuf {
    crate::goal_board_store::store_path(&simard_state_root())
}

/// Canonical path of the cross-process goal-board advisory `flock` file for a
/// given `state_root`. Both [`crate::goal_curation::save_goal_board`] and
/// [`crate::goal_board_store`] rendezvous on this one file (issue #2511) so the
/// daemon's snapshot flush and a concurrent `simard goal` CLI mutation cannot
/// interleave.
pub fn goal_board_lock_path_in(state_root: &Path) -> PathBuf {
    state_root.join("state").join("goal-board.lock")
}

/// [`goal_board_lock_path_in`] rooted at the resolved [`simard_state_root`].
pub fn goal_board_lock_path() -> PathBuf {
    goal_board_lock_path_in(&simard_state_root())
}

/// Look up `SIMARD_STATE_ROOT` and return `Some(path)` only if it passes the
/// validation rules (non-empty, absolute, NUL-free). Emits a one-shot WARN
/// the first time a malformed value is observed so operators can fix it.
fn sanitized_env_state_root() -> Option<PathBuf> {
    let raw = std::env::var_os(STATE_ROOT_ENV)?;
    let s = raw.to_string_lossy();
    let trimmed = s.trim();

    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('\0') {
        warn_once_invalid_state_root("contains NUL byte");
        return None;
    }

    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        warn_once_invalid_state_root("not absolute");
        return None;
    }

    Some(path)
}

/// Fallback when no valid `SIMARD_STATE_ROOT` is present.
fn home_default() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(DEFAULT_STATE_ROOT_DIRNAME);
    }
    // dirs::home_dir() is a slower fallback for non-HOME platforms.
    if let Some(home) = dirs::home_dir() {
        return home.join(DEFAULT_STATE_ROOT_DIRNAME);
    }
    // Last-resort relative default; never panics. Operators will see the
    // resulting path in tracing and can correct it.
    PathBuf::from(".").join(DEFAULT_STATE_ROOT_DIRNAME)
}

fn warn_once_invalid_state_root(reason: &'static str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_ok() {
        warn!(
            env_var = STATE_ROOT_ENV,
            reason = reason,
            "SIMARD_STATE_ROOT ignored; falling back to default"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Helper: scoped env override that resets on drop. Used because env
    /// access is process-global and parallel tests would race.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: tests in this module are serialized via #[serial].
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: tests in this module are serialized via #[serial].
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests in this module are serialized via #[serial].
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial(simard_state_root_env, cognitive_memory)]
    fn absolute_env_var_wins() {
        let _g = EnvGuard::set(STATE_ROOT_ENV, "/tmp/simard-state-root-test");
        assert_eq!(
            simard_state_root(),
            PathBuf::from("/tmp/simard-state-root-test")
        );
        assert_eq!(
            resolve_subdir("meetings"),
            PathBuf::from("/tmp/simard-state-root-test/meetings")
        );
    }

    #[test]
    #[serial(simard_state_root_env, cognitive_memory)]
    fn empty_env_var_falls_back_to_default() {
        let _g = EnvGuard::set(STATE_ROOT_ENV, "");
        let resolved = simard_state_root();
        assert_ne!(resolved.as_os_str(), "");
        // Default ends in `.simard`.
        assert!(
            resolved.ends_with(DEFAULT_STATE_ROOT_DIRNAME),
            "expected default to end in {DEFAULT_STATE_ROOT_DIRNAME}, got {resolved:?}"
        );
    }

    #[test]
    #[serial(simard_state_root_env, cognitive_memory)]
    fn relative_env_var_is_ignored() {
        let _g = EnvGuard::set(STATE_ROOT_ENV, "relative/path");
        let resolved = simard_state_root();
        // Falls through to default; default is absolute on any reasonable
        // platform (HOME / dirs::home_dir / `./` last resort all start at a
        // known root).
        assert!(
            !resolved.ends_with("relative/path"),
            "relative env var should be rejected, got {resolved:?}"
        );
    }

    #[test]
    #[serial(simard_state_root_env, cognitive_memory)]
    fn unset_env_var_falls_back_to_home_simard() {
        let _g = EnvGuard::unset(STATE_ROOT_ENV);
        let resolved = simard_state_root();
        assert!(
            resolved.ends_with(DEFAULT_STATE_ROOT_DIRNAME),
            "default should end in {DEFAULT_STATE_ROOT_DIRNAME}, got {resolved:?}"
        );
    }

    #[test]
    #[serial(simard_state_root_env, cognitive_memory)]
    fn resolve_subdir_concatenates_under_root() {
        let _g = EnvGuard::set(STATE_ROOT_ENV, "/tmp/simard-rs-subdir-test");
        assert_eq!(
            resolve_subdir("meeting_handoffs"),
            PathBuf::from("/tmp/simard-rs-subdir-test/meeting_handoffs")
        );
        assert_eq!(
            resolve_subdir("goals"),
            PathBuf::from("/tmp/simard-rs-subdir-test/goals")
        );
    }
}
