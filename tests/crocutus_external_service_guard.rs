//! Integration coverage for Step 8b: the observe-only (read-only) floor must be
//! enforced at the **external-service** command seam (`crate::guarded_command`),
//! not just for `git`.
//!
//! The Crocutus identity sets `SIMARD_OBSERVE_ONLY=1`. Under that flag every
//! mutating `gh`/`az`/HTTP command routed through the seam must be refused
//! *before the process is spawned*, while reads and the (unset-flag) engineer
//! identity are unaffected. These tests exercise the public seam API directly so
//! the guarantee is verified end-to-end, independent of any single call site.

use simard::guarded_command;
use simard::read_only_guard::OBSERVE_ONLY_ENV;
use std::io::ErrorKind;
use std::sync::Mutex;

// These tests mutate the process-global observe-only env var. Serialize them so
// the flag from one test never leaks into another. `serial(cognitive_memory)`
// is the crate-wide key already used by the other observe-only env tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_observe(on: bool) {
    unsafe {
        if on {
            std::env::set_var(OBSERVE_ONLY_ENV, "1");
        } else {
            std::env::remove_var(OBSERVE_ONLY_ENV);
        }
    }
}

#[serial_test::serial(cognitive_memory)]
#[test]
fn observe_only_refuses_external_service_writes_at_the_seam() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_observe(true);

    // Every one of these mutates a remote target; each must be refused with the
    // stable marker and a PermissionDenied kind — and never spawned.
    let writes: &[(&str, &[&str])] = &[
        ("gh", &["issue", "create", "--title", "t", "--body", "b"]),
        ("gh", &["pr", "create", "--title", "t", "--body", "b"]),
        ("gh", &["pr", "merge", "42", "--squash"]),
        ("gh", &["issue", "comment", "42", "--body", "hi"]),
        ("gh", &["issue", "close", "42"]),
        ("gh", &["api", "-X", "POST", "/repos/o/r/issues"]),
        ("az", &["repos", "pr", "create", "--title", "t"]),
        ("az", &["boards", "work-item", "update", "--id", "1"]),
        (
            "az",
            &["rest", "--method", "POST", "--uri", "https://example.test"],
        ),
        (
            "curl",
            &["-X", "POST", "-d", "payload", "https://example.test/x"],
        ),
    ];

    for (program, args) in writes {
        let err = guarded_command::run_output(program, args).expect_err(&format!(
            "{program} {args:?} must be refused under observe-only"
        ));
        assert_eq!(
            err.kind(),
            ErrorKind::PermissionDenied,
            "{program} {args:?} must fail with PermissionDenied, got {err:?}",
        );
        assert!(
            err.to_string().contains("GUARDRAIL BLOCKED"),
            "{program} {args:?} error must carry the stable marker, got: {err}",
        );
        // `screen` (the no-spawn variant used by incremental/async sites) must
        // agree, prepending the program to form the full argv.
        let mut full = vec![*program];
        full.extend_from_slice(args);
        assert!(
            guarded_command::screen(&full).is_err(),
            "screen must also refuse {full:?}",
        );
    }

    set_observe(false);
}

#[serial_test::serial(cognitive_memory)]
#[test]
fn observe_only_permits_reads_so_the_observer_can_still_see() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_observe(true);

    // Reads are the whole point of an observer — they must pass the seam.
    let reads: &[&[&str]] = &[
        &["gh", "issue", "list", "--state", "open"],
        &["gh", "pr", "view", "42", "--json", "state"],
        &["gh", "api", "/repos/o/r/issues"],
        &["az", "repos", "pr", "list"],
        &[
            "az",
            "rest",
            "--method",
            "GET",
            "--uri",
            "https://example.test",
        ],
        &["curl", "https://example.test/x"],
    ];
    for argv in reads {
        assert!(
            guarded_command::screen(argv).is_ok(),
            "observe-only must permit the read {argv:?}",
        );
    }

    set_observe(false);
}

#[serial_test::serial(cognitive_memory)]
#[test]
fn engineer_identity_is_unaffected_when_flag_unset() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_observe(false);

    // With the flag unset (Simard, the engineering identity), the seam is a
    // transparent pass-through: even mutating commands screen Ok. `screen` does
    // not spawn, so this asserts the gate is a no-op without side effects.
    for argv in [
        vec!["gh", "issue", "create", "--title", "t"],
        vec!["gh", "pr", "merge", "42", "--squash"],
        vec!["az", "repos", "pr", "create"],
    ] {
        assert!(
            guarded_command::screen(&argv).is_ok(),
            "engineer identity must not be blocked: {argv:?}",
        );
    }
}

#[test]
fn seam_actually_spawns_permitted_commands() {
    // Regardless of identity, an out-of-scope tool (`echo`) is permitted; this
    // proves the seam runs the process and returns its captured output rather
    // than only screening. `echo` is not one of the screened external services.
    let out = guarded_command::run_output("echo", &["step-8b-ok"]).expect("echo must run");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "step-8b-ok",
        "the seam must return the spawned process output",
    );
}
