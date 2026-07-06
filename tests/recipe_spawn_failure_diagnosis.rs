//! Pre-exec spawn-failure diagnosis for the journal (and every Tier-A) recipe
//! spawn (issues #2640/#2692) — the "no silent fallback" half of the fix.
//!
//! The journal's E2BIG is an `io::Error` from `Command::output()` that fails
//! BEFORE the child exists, so it never produces an `ExitStatus` and the #2640
//! `classify_terminal_failure(status, transcript)` seam cannot see it. The fix
//! adds a sibling, errno-keyed classifier `classify_spawn_failure(&io::Error)`
//! and records its `FailureDiagnosis` into `overseer::failure_sink` so the
//! Overseer can act — instead of the old bare `tracing::warn!` + silent raw-dump
//! fallback that produced "a journal full of raw error dumps".
//!
//! These tests pin that classifier and the sink wiring hermetically, using only
//! public API and synthetic `io::Error`s.
//!
//! TDD status: RED until the fix adds
//! `simard::overseer::diagnosis::classify_spawn_failure`. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

use std::io::Error as IoError;

use simard::overseer::diagnosis::{FailureCause, MAX_EVIDENCE_LEN, classify_spawn_failure};
use simard::overseer::failure_sink::{drain_recent, record_step_failure};

/// The exact live defect: `E2BIG` (`errno 7`, "Argument list too long") from the
/// recipe spawn must classify as `ArgListTooLong`, and — because there was no
/// child — carry NO exit code.
#[test]
fn e2big_errno_classifies_as_arg_list_too_long() {
    let err = IoError::from_raw_os_error(7);
    let diag = classify_spawn_failure(&err);

    assert_eq!(
        diag.cause,
        FailureCause::ArgListTooLong,
        "errno 7 (E2BIG) must diagnose as ArgListTooLong — the journal spawn defect"
    );
    assert_eq!(
        diag.exit_code, None,
        "a pre-exec spawn failure has no ExitStatus, so exit_code must be None"
    );
    assert!(
        !diag.evidence.is_empty(),
        "evidence must carry the errno-derived error string, never an empty drop"
    );
}

/// A full disk at the temp-file write step surfaces as `ENOSPC` (`errno 28`) and
/// must classify as `DiskFull` — reusing the existing cause, no new variant.
#[test]
fn enospc_errno_classifies_as_disk_full() {
    let diag = classify_spawn_failure(&IoError::from_raw_os_error(28));
    assert_eq!(diag.cause, FailureCause::DiskFull);
    assert_eq!(diag.exit_code, None);
}

/// `ENOMEM` (`errno 12`) at spawn must classify as `OutOfMemory`.
#[test]
fn enomem_errno_classifies_as_out_of_memory() {
    let diag = classify_spawn_failure(&IoError::from_raw_os_error(12));
    assert_eq!(diag.cause, FailureCause::OutOfMemory);
    assert_eq!(diag.exit_code, None);
}

/// An unrecognised errno must still be recorded structurally as `Unknown` — the
/// classifier never panics and never silently drops (e.g. `ENOENT`, errno 2).
#[test]
fn unrecognised_errno_classifies_as_unknown() {
    let diag = classify_spawn_failure(&IoError::from_raw_os_error(2));
    assert_eq!(
        diag.cause,
        FailureCause::Unknown,
        "an unmapped errno must still yield a structured Unknown diagnosis, not a drop"
    );
}

/// Platforms/errors that do not surface a numeric `raw_os_error()` must still be
/// classified via the message string fallback ("argument list too long").
#[test]
fn arg_list_too_long_message_fallback_without_errno() {
    let err = IoError::other("exec failed: Argument list too long");
    assert_eq!(err.raw_os_error(), None, "precondition: no numeric errno");

    let diag = classify_spawn_failure(&err);
    assert_eq!(
        diag.cause,
        FailureCause::ArgListTooLong,
        "the string fallback must catch 'Argument list too long' when errno is absent"
    );
}

/// Evidence must be bounded so a pathological error string can never inflate a
/// log line, notification, or the sink.
#[test]
fn evidence_is_bounded() {
    let huge = "x".repeat(10_000);
    let err = IoError::other(huge);
    let diag = classify_spawn_failure(&err);
    assert!(
        diag.evidence.chars().count() <= MAX_EVIDENCE_LEN,
        "evidence must be bounded at MAX_EVIDENCE_LEN ({MAX_EVIDENCE_LEN}), got {}",
        diag.evidence.chars().count()
    );
}

/// No silent fallback: an injected spawn `Err` at a Tier-A site — modelled here
/// as the exact record-on-failure call the journal makes — must land EXACTLY one
/// `ArgListTooLong` diagnosis in the Overseer sink for the next Observe pass to
/// act on. This is the whole point: the E2BIG is surfaced, not swallowed.
///
/// Kept in a single test function because `failure_sink` is a process-global
/// buffer; no other test in this binary touches it.
#[test]
fn tier_a_spawn_failure_is_recorded_not_swallowed() {
    // Clear anything left in the process-global sink first.
    let _ = drain_recent();

    // The journal's spawn-failure arm records the classified diagnosis before
    // it propagates/degrades.
    let e2big = IoError::from_raw_os_error(7);
    record_step_failure(classify_spawn_failure(&e2big));

    let drained = drain_recent();
    assert_eq!(
        drained.len(),
        1,
        "exactly one diagnosis must be recorded for the Overseer to act on — \
         the failure must NOT be dropped into a bare warn!"
    );
    assert_eq!(
        drained[0].cause,
        FailureCause::ArgListTooLong,
        "the recorded diagnosis must be the E2BIG cause"
    );

    // The sink must be empty after draining (drain semantics).
    assert!(
        drain_recent().is_empty(),
        "drain_recent must empty the sink so a diagnosis is acted on once"
    );
}
