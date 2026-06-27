//! Tests for [`super::orphan`]: the conservative engineer-orphan matching rule
//! (pure, fully covered now) and the effectful reaper (idempotent empty case
//! covered now; signal/`/proc` paths pinned by `#[ignore]`d tests).

use std::path::Path;

use super::orphan::{
    OrphanEngineer, find_engineer_orphans, match_engineer_orphan, reap_engineer_orphans,
};

const INSTALL: &str = "/home/simard/.simard/bin/simard";

#[test]
fn matches_engineer_run_orphan_at_install_path() {
    assert!(match_engineer_orphan(
        Path::new(INSTALL),
        Path::new(INSTALL),
        "/home/simard/.simard/bin/simard engineer run --goal g1",
        4242,
        1, // self
        None,
    ));
}

#[test]
fn excludes_self_pid() {
    assert!(!match_engineer_orphan(
        Path::new(INSTALL),
        Path::new(INSTALL),
        "simard engineer run --goal g1",
        1,
        1, // self == pid
        None,
    ));
}

#[test]
fn excludes_incoming_daemon_pid() {
    assert!(!match_engineer_orphan(
        Path::new(INSTALL),
        Path::new(INSTALL),
        "simard engineer run --goal g1",
        9001,
        1,
        Some(9001), // new daemon
    ));
}

#[test]
fn excludes_other_executable_paths() {
    // Same argv shape, different executable — must NOT be killed.
    assert!(!match_engineer_orphan(
        Path::new(INSTALL),
        Path::new("/usr/local/bin/simard"),
        "simard engineer run --goal g1",
        4242,
        1,
        None,
    ));
}

#[test]
fn excludes_non_engineer_run_invocations() {
    // The daemon itself / unrelated simard subcommands are not orphans.
    for argv in [
        "simard ooda run",
        "simard engineer", // bare token, no `run`
        "simard run engineer",
        "simard self-test",
        "simard engineer status",
    ] {
        assert!(
            !match_engineer_orphan(Path::new(INSTALL), Path::new(INSTALL), argv, 4242, 1, None),
            "argv {argv:?} must not match"
        );
    }
}

#[test]
fn matches_engineer_run_with_extra_leading_and_trailing_args() {
    assert!(match_engineer_orphan(
        Path::new(INSTALL),
        Path::new(INSTALL),
        "  simard   engineer run   --goal g7 --repo Simard ",
        4242,
        1,
        None,
    ));
}

#[test]
fn reap_empty_set_is_idempotent_success() {
    // Idempotent: no matches is success, with zero side effects.
    assert_eq!(reap_engineer_orphans(&[], 10).unwrap(), 0);
}

// --- Effectful paths: real, hermetic coverage -------------------------------

#[test]
fn find_engineer_orphans_excludes_self_and_returns_vec() {
    // The current test process is NOT exec'ing the install path with
    // `engineer run`, so it must never appear in the scan; self is excluded by
    // pid in any case. The scan must not error on the live /proc table.
    let found = find_engineer_orphans(Path::new(INSTALL), std::process::id() as i32, None).unwrap();
    let me = std::process::id() as i32;
    assert!(
        found.iter().all(|o| o.pid != me),
        "scan must never return the current pid"
    );
}

#[test]
fn reap_terminates_a_real_child_then_returns_count() {
    use std::process::Command;
    // Spawn a real, harmless child we are allowed to signal.
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id() as i32;
    let orphan = OrphanEngineer {
        pid,
        cmdline: "sleep 30".to_string(),
    };

    let reaped = reap_engineer_orphans(&[orphan], 2).unwrap();
    assert_eq!(reaped, 1, "one orphan handled");

    // The child must be gone; wait so no zombie lingers in the OS table.
    let _ = child.wait();
}

#[test]
fn reap_ignores_non_positive_pids_without_broadcasting() {
    // pid 0 / negative would target a process *group* or broadcast under
    // `libc::kill` — the reaper must treat them as already-gone and never
    // signal a group.
    let orphans = vec![
        OrphanEngineer {
            pid: 0,
            cmdline: "simard engineer run".to_string(),
        },
        OrphanEngineer {
            pid: -1,
            cmdline: "simard engineer run".to_string(),
        },
    ];
    let reaped = reap_engineer_orphans(&orphans, 1).unwrap();
    assert_eq!(reaped, 2);
}
