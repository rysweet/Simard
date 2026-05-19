//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for the
//! bridge-launcher's interaction with the new per-state-root socket
//! resolution.
//!
//! The contract under test is the end-to-end hermeticity property:
//! when a test sets `SIMARD_STATE_ROOT=<tempdir>` and unsets
//! `SIMARD_MEMORY_SOCKET`, [`launch_writer_bridge`] must NOT connect
//! to the operator's live daemon socket at `~/.simard/memory.sock` —
//! even when that daemon is currently running. The launcher must
//! resolve its tier-1 socket via [`socket_path_for`] against the
//! requested `state_root`, which lives inside the TempDir.
//!
//! These tests cover the gap that the #1923/#1925 forensics exposed:
//! `SIMARD_STATE_ROOT`-set tests historically still wrote into the
//! operator's live `cognitive_memory.ladybug` because the IPC tier
//! short-circuited via the hard-coded `default_socket_path()`. The
//! socket-path-per-state-root fix is what makes pointing the env var
//! at a TempDir actually isolate.
//!
//! Tests fail until the implementation step migrates
//! `launch_writer_bridge` / `open_reader_bridge` to call
//! `socket_path_for(state_root)` instead of `default_socket_path()`.

use std::path::PathBuf;

use serial_test::serial;

use super::{MEMORY_SOCKET_ENV, launch_writer_bridge, open_reader_bridge};
use crate::state_root::STATE_ROOT_ENV;

/// Local env-guard so this file does not depend on the not-yet-built
/// `HermeticState` helper.
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

fn home_simard_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into());
    PathBuf::from(home).join(".simard")
}

#[test]
#[serial(cognitive_memory)]
fn launch_writer_bridge_with_tempdir_state_root_does_not_touch_home_simard() {
    // The headline #1923/#1925 property: hermeticity must hold even
    // when ~/.simard/memory.sock exists (e.g. the operator's live
    // daemon is up). We approximate that condition by simply not
    // requiring its absence — the assertion below is on what the
    // launcher *writes*, not what it *reads*.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _state = EnvGuard::set(STATE_ROOT_ENV, root.to_str().unwrap());
    let _sock = EnvGuard::unset(MEMORY_SOCKET_ENV);

    let bridge = launch_writer_bridge(root)
        .expect("launch_writer_bridge must succeed against a fresh TempDir state root");

    // Write a uniquely-tagged fact so we can prove which DB landed it.
    let tag = format!(
        "tdd-1923-isolation:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    bridge
        .ops()
        .store_fact(
            &tag,
            "must land in TempDir DB, not ~/.simard",
            1.0,
            &["tdd-1923".to_string()],
            "tdd-isolation",
        )
        .expect("store_fact must succeed through the hermetic writer");

    // Property 1: the TempDir's DB file or its WAL exists after the
    // write — proves the writer landed inside the TempDir.
    let tempdir_db_artifact_present = std::fs::read_dir(root)
        .expect("state root readable")
        .filter_map(|e| e.ok())
        .any(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.starts_with("cognitive_memory.ladybug")
        });
    assert!(
        tempdir_db_artifact_present,
        "after a writer bridge round-trip against state_root={}, at \
         least one cognitive_memory.ladybug* file must exist there. \
         If none exists, the writer routed to a different DB — the \
         #1923/#1925 leak.",
        root.display(),
    );

    // Property 2: the bridge must NOT have written to ~/.simard. Inspect
    // the operator's live DB only via its facts view, looking for our
    // unique tag. If the tag shows up there, the writer leaked.
    let home_simard = home_simard_path();
    let live_state = home_simard.join("state");
    if let Ok(live_bridge) = open_reader_bridge(&live_state) {
        let hits = live_bridge
            .ops()
            .search_facts(&tag, 4, 0.0)
            .unwrap_or_default();
        assert!(
            hits.is_empty(),
            "TempDir-rooted writer must NOT have written into \
             {}; tag {} was found there. This is the #1923/#1925 \
             fixture leak — the bridge connected to the live \
             daemon's socket via the hard-coded default path.",
            live_state.display(),
            tag,
        );
    }
}

#[test]
#[serial(cognitive_memory)]
fn writer_and_reader_bridge_round_trip_within_hermetic_state_root() {
    // Symmetric round-trip: writer + reader against the SAME hermetic
    // state root must observe the fact. Catches a regression where the
    // socket follows the state root but the reader still falls through
    // to the wrong DB.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _state = EnvGuard::set(STATE_ROOT_ENV, root.to_str().unwrap());
    let _sock = EnvGuard::unset(MEMORY_SOCKET_ENV);

    let tag = format!(
        "tdd-1923-roundtrip:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );

    {
        let writer = launch_writer_bridge(root).expect("writer bridge against hermetic state root");
        writer
            .ops()
            .store_fact(
                &tag,
                "hermetic round-trip payload",
                1.0,
                &["tdd-1923".to_string()],
                "tdd-isolation",
            )
            .expect("store_fact");
    }

    // After dropping the writer, opening a reader against the same root
    // must surface the same tag.
    let reader = open_reader_bridge(root).expect("reader bridge against hermetic state root");
    let hits = reader
        .ops()
        .search_facts(&tag, 4, 0.0)
        .expect("search_facts on hermetic reader");
    assert!(
        hits.iter().any(|f| f.concept == tag),
        "round-trip fact {} not visible to a reader on the same \
         hermetic state root {} — likely indicates the writer landed \
         in a different DB than the reader is reading",
        tag,
        root.display(),
    );
}
