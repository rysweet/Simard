//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for the
//! `cfg(test)`-only hermetic-state-root guard.
//!
//! Contract under test (see `docs/testing/hermetic-tests.md`):
//!
//! The guard runs at three independent enforcement sites:
//!   1. `goals::persistence::save_goal_board` (and
//!      `save_goal_board_with_removals`)
//!   2. `cognitive_memory::native::NativeCognitiveMemory::store_fact`
//!      (and its `store_episode` / `store_procedure` siblings)
//!   3. `memory_ipc::launcher::launch_writer_bridge`
//!
//! Each site asserts:
//!   - `default_state_root()` is under `env::temp_dir()`.
//!   - `default_state_root()` is NOT under `$HOME/.simard`.
//!   - Unless `SIMARD_TEST_ALLOW_LIVE_STATE=1`.
//!
//! Tripping the guard fails the test with a message that names the
//! offending path and points to `docs/testing/hermetic-tests.md`.
//!
//! Tests fail until the implementation step adds the guard at the three
//! sites and the supporting helper module.
//!
//! ## What these tests assert
//!
//! 1. **Positive path**: with `SIMARD_STATE_ROOT` set to a TempDir, the
//!    guard is a no-op (writes succeed).
//! 2. **Negative path**: with `SIMARD_STATE_ROOT` removed AND `HOME`
//!    repointed at a writable temp dir (so the fallback
//!    `$HOME/.simard/state` resolves under temp_dir but `is_under_home_simard`
//!    returns true), every guarded write panics with the documented
//!    message.
//! 3. **Opt-out**: with `SIMARD_TEST_ALLOW_LIVE_STATE=1`, the guard
//!    short-circuits and the negative path becomes a no-op.
//! 4. **Three sites**: the guard fires from all of
//!    `save_goal_board` / `save_goal_board_with_removals`,
//!    `store_fact`, and `launch_writer_bridge`.
//!
//! The tests deliberately do NOT exercise an absolute "outside temp_dir"
//! path because that would require writing into a real location on the
//! host filesystem, which would defeat the very property we are
//! testing. Instead they exercise the under-`HOME/.simard` failure mode
//! via a controlled `HOME` repointing — the same mechanism the install
//! harness uses.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use serial_test::serial;
use tempfile::TempDir;

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, add_active_goal, save_goal_board,
    save_goal_board_with_removals,
};
use crate::memory_ipc::{TEST_ALLOW_LIVE_STATE_ENV, launch_writer_bridge};
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

fn sample_board() -> GoalBoard {
    let mut b = GoalBoard::new();
    add_active_goal(
        &mut b,
        ActiveGoal {
            id: "tdd-guard".into(),
            description: "tdd-guard sample".into(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        },
    )
    .unwrap();
    b
}

/// Stage a fake `$HOME` so that the default state-root fallback
/// (`$HOME/.simard/state`) lands under `home_dir` — which itself
/// becomes the target of the guard's under-HOME/.simard check. Removes
/// `SIMARD_STATE_ROOT` so the fallback path is exercised. Returns the
/// resolved state_root for downstream assertions and the temp dir to
/// keep alive.
fn fake_home_under_temp() -> (TempDir, PathBuf, PathBuf) {
    let home_tmp = tempfile::tempdir().expect("home tempdir");
    let home = home_tmp.path().to_path_buf();
    let state_root = home.join(".simard").join("state");
    std::fs::create_dir_all(&state_root).expect("create fake state root");
    (home_tmp, home, state_root)
}

// ─── positive path: hermetic state root → no panic ─────────────────────────

#[test]
#[serial(cognitive_memory)]
fn save_goal_board_against_tempdir_state_root_does_not_trip_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _state = EnvGuard::set(STATE_ROOT_ENV, tmp.path().to_str().unwrap());
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let board = sample_board();
    let bridge =
        launch_writer_bridge(tmp.path()).expect("writer bridge against hermetic state root");
    save_goal_board(&board, bridge.ops())
        .expect("save_goal_board against TempDir state root must succeed");
}

#[test]
#[serial(cognitive_memory)]
fn save_goal_board_with_removals_against_tempdir_state_root_does_not_trip_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _state = EnvGuard::set(STATE_ROOT_ENV, tmp.path().to_str().unwrap());
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let board = sample_board();
    let bridge = launch_writer_bridge(tmp.path()).expect("writer bridge");
    save_goal_board_with_removals(&board, &[], bridge.ops())
        .expect("save_goal_board_with_removals against TempDir state root must succeed");
}

// ─── negative path: HOME-rooted default state root → panic ─────────────────

#[test]
#[serial(cognitive_memory)]
fn save_goal_board_against_home_simard_state_root_trips_guard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let board = sample_board();

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        // Open a bridge by-path so we can exercise the save site
        // specifically. The launcher also has its own guard fire site
        // (see the dedicated test below) — to isolate the save site we
        // bypass the launcher's guard via the test-only constructor.
        let mem =
            NativeCognitiveMemory::open(&state_root).expect("open DB at fake home state root");
        let bridge = crate::memory_ipc::WriterBridge::from_ops_for_test(Box::new(mem));
        let _ = save_goal_board(&board, bridge.ops());
    }));
    assert!(
        panicked.is_err(),
        "save_goal_board MUST panic when default_state_root() resolves \
         under HOME/.simard ({}). The hermetic-state-root guard is the \
         #1923/#1925 regression prevention; see \
         docs/testing/hermetic-tests.md.",
        state_root.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn store_fact_against_home_simard_state_root_trips_guard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mem = NativeCognitiveMemory::open(&state_root).expect("open DB");
        let _ = mem.store_fact(
            "tdd-guard:store-fact",
            "should be guard-rejected",
            1.0,
            &["tdd-guard".to_string()],
            "tdd-guard",
        );
    }));
    assert!(
        panicked.is_err(),
        "store_fact MUST panic when default_state_root() resolves under \
         HOME/.simard ({}). All three guard sites must fire \
         independently so deleting one does not silently disable the \
         protection.",
        state_root.display(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn launch_writer_bridge_against_home_simard_state_root_trips_guard() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::unset(TEST_ALLOW_LIVE_STATE_ENV);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = launch_writer_bridge(&state_root);
    }));
    assert!(
        panicked.is_err(),
        "launch_writer_bridge MUST panic when state_root is under \
         HOME/.simard ({}). The launcher fires the guard before \
         returning a writer regardless of the tier selected.",
        state_root.display(),
    );
}

// ─── opt-out: SIMARD_TEST_ALLOW_LIVE_STATE=1 silences the guard ────────────

#[test]
#[serial(cognitive_memory)]
fn allow_live_state_env_silences_guard_for_install_harness_use() {
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::set(TEST_ALLOW_LIVE_STATE_ENV, "1");

    let board = sample_board();

    // With the opt-out env var set, the guard must short-circuit and
    // the write must complete normally.
    let bridge = launch_writer_bridge(&state_root)
        .expect("with SIMARD_TEST_ALLOW_LIVE_STATE=1, launch must succeed");
    save_goal_board(&board, bridge.ops())
        .expect("with SIMARD_TEST_ALLOW_LIVE_STATE=1, save must succeed");
}

#[test]
#[serial(cognitive_memory)]
fn allow_live_state_env_only_value_one_silences_guard() {
    // Sanity: arbitrary truthy strings must NOT silence the guard.
    // Only the exact value "1" is documented as the opt-out signal.
    let (_home_tmp, home, state_root) = fake_home_under_temp();
    let _home_guard = EnvGuard::set("HOME", home.to_str().unwrap());
    let _state_unset = EnvGuard::unset(STATE_ROOT_ENV);
    let _allow = EnvGuard::set(TEST_ALLOW_LIVE_STATE_ENV, "yes");

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = launch_writer_bridge(&state_root);
    }));
    assert!(
        panicked.is_err(),
        "SIMARD_TEST_ALLOW_LIVE_STATE=yes must NOT silence the guard; \
         only the exact value '1' is the documented opt-out signal"
    );
}
