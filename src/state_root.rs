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

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::OnceLock;

use tracing::warn;

/// Environment variable that relocates the durable state root for the whole
/// CLI (meetings, handoffs, goal board, future cognitive backups).
pub const STATE_ROOT_ENV: &str = "SIMARD_STATE_ROOT";

/// Default state-root directory name under `$HOME` when no env override is
/// present. Lifted out of the function to keep the constant single-sourced.
pub const DEFAULT_STATE_ROOT_DIRNAME: &str = ".simard";

thread_local! {
    /// Per-thread override of the resolved durable state root.
    ///
    /// The process-global `SIMARD_STATE_ROOT` env var is the production source
    /// of truth, but it makes test isolation fragile: tests redirect the state
    /// root by mutating this single global, and `serial_test` only serializes
    /// tests that share the *same* key. A test reading the state root lazily
    /// (e.g. an axum dashboard handler calling [`simard_state_root`] /
    /// `resolve_state_root`) could therefore observe a value written by a
    /// concurrently-running test that used a different serial key — the
    /// read-after-write race behind the flaky `full_goal_lifecycle_crud`
    /// (issue #2320).
    ///
    /// `HermeticState` installs this thread-local for the lifetime of the
    /// helper. Because `#[tokio::test]` runs on a current-thread runtime, the
    /// code under test executes on the same OS thread that constructed the
    /// helper, so it reads this thread's pinned root regardless of what any
    /// other thread does to the global env var. The override is never
    /// installed in production, so the resolvers fall straight through to the
    /// env-var ladder there.
    static STATE_ROOT_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Install (or clear) the calling thread's state-root override, returning the
/// previous value so callers can restore it on drop.
///
/// Test-support plumbing for [`crate::test_support::HermeticState`]; not used
/// by production code paths.
pub(crate) fn set_thread_state_root_override(path: Option<PathBuf>) -> Option<PathBuf> {
    STATE_ROOT_OVERRIDE.with(|cell| cell.replace(path))
}

/// Read the calling thread's state-root override, if one is installed.
pub(crate) fn thread_state_root_override() -> Option<PathBuf> {
    STATE_ROOT_OVERRIDE.with(|cell| cell.borrow().clone())
}

/// Resolve the durable state-root directory.
///
/// Returns the first valid match from the ladder in the module-level docs.
/// Never panics; never creates the directory. The first writer is responsible
/// for `create_dir_all` on the resolved subdirectory.
pub fn simard_state_root() -> PathBuf {
    // A per-thread test override (installed by `HermeticState`) wins over the
    // process-global env var so a cognitive-memory test cannot be derailed by
    // another test thread mutating `SIMARD_STATE_ROOT` concurrently. In
    // production the override is never installed, so this is a no-op (issue
    // #2320).
    if let Some(p) = thread_state_root_override() {
        return p;
    }
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

    /// Issue #2320: a per-thread override (installed by `HermeticState`) must
    /// win over the process-global `SIMARD_STATE_ROOT` env var, so a test's
    /// lazily-resolving callers stay pinned to its hermetic root even when
    /// another thread has mutated the env var to something else.
    #[test]
    #[serial(simard_state_root_env)]
    fn thread_override_beats_env_var() {
        let _g = EnvGuard::set(STATE_ROOT_ENV, "/tmp/simard-env-root-2320");
        let prev = set_thread_state_root_override(Some(PathBuf::from("/tmp/simard-override-2320")));

        let resolved = simard_state_root();

        // Restore the override before asserting so a failure cannot leak into
        // a later test reusing this OS thread.
        set_thread_state_root_override(prev);

        assert_eq!(
            resolved,
            PathBuf::from("/tmp/simard-override-2320"),
            "thread-local override must take precedence over SIMARD_STATE_ROOT"
        );
        // With the override cleared, resolution falls back to the env var.
        assert_eq!(
            simard_state_root(),
            PathBuf::from("/tmp/simard-env-root-2320"),
            "clearing the override must restore env-var resolution"
        );
    }
}
