//! Contract tests for the process-group orphan guard (issue: Simard
//! nested-subprocess supervision hardening; cross-links `amplihack-rs#964`).
//!
//! These tests pin the teardown contract for [`GroupChild`] and the
//! [`ProcessGroupProbe`] signalling seam:
//! * the two behavioural teardown tests assert the SIGTERM → bounded grace →
//!   SIGKILL escalation order (and that SIGKILL is never led with);
//! * the remaining tests pin the safety invariants — never signal a
//!   non-positive pgid; never signal a disarmed or reaped guard.
//!
//! A real end-to-end proof that no orphan survives a failed/aborted run lives
//! in `tests/process_group_orphan_reaping.rs`.
//!
//! Design constraints honoured: offline, sleep-free (grace = `Duration::ZERO`
//! so the escalation loop makes a single liveness check and never sleeps), and
//! process-free for the pure teardown tests (a fake pgid + injected
//! [`RecordingProbe`] replace any real process group). The one test that needs
//! a real short-lived child is `#[cfg(unix)]` + serial and never sleeps.

use std::sync::Arc;
use std::time::Duration;

use super::group_child::GroupChild;
use super::probe::{LibcSignaller, ProcessGroupProbe, RecordingProbe};

/// SIGTERM as the guard will pass it to the signaller.
fn sigterm() -> i32 {
    libc::SIGTERM
}

/// SIGKILL as the guard will pass it to the signaller.
fn sigkill() -> i32 {
    libc::SIGKILL
}

// ---------------------------------------------------------------------------
// (d) Core behaviour — full-subtree teardown on drop.
// ---------------------------------------------------------------------------

/// A child that ignores SIGTERM (its group is still alive after the graceful
/// signal) MUST be escalated: the guard signals the whole group with SIGTERM
/// first and then SIGKILL — in that order, never leading with SIGKILL.
#[test]
fn drop_signals_whole_group_then_escalates_when_group_survives_sigterm() {
    let probe = Arc::new(RecordingProbe::new());
    probe.set_alive(true); // group ignores SIGTERM -> escalation required
    let pgid = 4242;

    {
        // grace = ZERO so the escalation loop makes a single liveness check and
        // never sleeps in tests.
        let _guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
        // guard dropped here (models an error/abort/timeout/panic exit path)
    }

    assert_eq!(
        probe.recorded(),
        vec![(pgid, sigterm()), (pgid, sigkill())],
        "armed drop must SIGTERM then SIGKILL the whole group when it survives SIGTERM"
    );
}

/// A child whose group exits on the graceful SIGTERM MUST NOT be SIGKILLed —
/// the guard signals SIGTERM only. Escalation is a last resort.
#[test]
fn drop_signals_only_sigterm_when_group_exits_gracefully() {
    let probe = Arc::new(RecordingProbe::new());
    probe.set_alive(false); // group dies on SIGTERM -> no escalation
    let pgid = 7000;

    {
        let _guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
    }

    assert_eq!(
        probe.recorded(),
        vec![(pgid, sigterm())],
        "a group that exits on SIGTERM must not be escalated to SIGKILL"
    );
}

/// Escalation discipline: whatever else happens, the FIRST signal a guard ever
/// sends is SIGTERM — never SIGKILL. (Redundant with the ordered vectors above
/// but pins the invariant on its own so a future refactor cannot regress it.)
#[test]
fn drop_never_leads_with_sigkill() {
    let probe = Arc::new(RecordingProbe::new());
    probe.set_alive(true);
    let pgid = 4243;

    {
        let _guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
    }

    let recorded = probe.recorded();
    assert_eq!(
        recorded.first().map(|(_, sig)| *sig),
        Some(sigterm()),
        "the first teardown signal must always be SIGTERM"
    );
}

// ---------------------------------------------------------------------------
// (a) Safety invariant — never signal a non-positive pgid (REQ-V1).
// ---------------------------------------------------------------------------

/// `pgid <= 1` is fail-closed: `0` targets the caller's own group and `1`
/// broadcasts. An armed guard with such a pgid MUST issue NO signal on drop,
/// even when the (scripted) group appears alive.
#[test]
fn drop_does_not_signal_when_pgid_not_positive() {
    for pgid in [0, 1, -1, i32::MIN] {
        let probe = Arc::new(RecordingProbe::new());
        probe.set_alive(true);

        {
            let _guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
        }

        assert!(
            probe.recorded().is_empty(),
            "pgid={pgid} is non-positive; the guard must never signal it (REQ-V1)"
        );
    }
}

// ---------------------------------------------------------------------------
// (b) Safety invariant — a disarmed guard never signals; the child survives.
// ---------------------------------------------------------------------------

/// After `disarm()` the caller owns the child; dropping the guard MUST tear
/// nothing down, even if the scripted group is still alive.
#[test]
fn disarmed_guard_is_never_signalled() {
    let probe = Arc::new(RecordingProbe::new());
    probe.set_alive(true);
    let pgid = 5555;

    {
        let mut guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
        let _ = guard.disarm(); // caller takes ownership (detached spawn)
    }

    assert!(
        probe.recorded().is_empty(),
        "a disarmed guard must not signal its group on drop"
    );
}

/// End-to-end ownership transfer: a real (short-lived, no sleep) child spawned
/// in its own group and then `disarm()`ed is handed back to the caller and
/// SURVIVES the guard's drop (no teardown signal is recorded). Serialised
/// because it touches real OS process state; never sleeps.
#[cfg(unix)]
#[test]
#[serial_test::serial(process_group_reaping)]
fn disarm_returns_child_ownership_and_suppresses_teardown() {
    use std::process::Command;

    let probe = Arc::new(RecordingProbe::new());
    // `true` exits immediately: a real child + real pgid, but no sleeping.
    let mut cmd = Command::new("true");
    let mut guard = GroupChild::spawn_with(&mut cmd, probe.clone(), Duration::ZERO)
        .expect("spawn short-lived child in its own group");

    assert!(guard.pgid() > 1, "a real child pgid must be > 1");

    let mut child = guard
        .disarm()
        .expect("disarm returns ownership of the spawned child");

    drop(guard); // must NOT signal — ownership was relinquished

    // The caller can still reap the child it now owns.
    let status = child.wait().expect("wait on disarmed child");
    assert!(status.success(), "`true` exits 0");

    assert!(
        probe.recorded().is_empty(),
        "a disarmed guard must not signal the group whose child it handed back"
    );
}

// ---------------------------------------------------------------------------
// (c) Safety invariant — a reaped guard is never re-signalled (REQ-V2).
// ---------------------------------------------------------------------------

/// Once a guard has been `reap()`ed, its pgid may have been recycled; dropping
/// it MUST NOT signal, to avoid killing an unrelated foreign group.
#[test]
fn reaped_guard_is_never_resignalled() {
    let probe = Arc::new(RecordingProbe::new());
    probe.set_alive(true);
    let pgid = 6666;

    {
        let mut guard = GroupChild::from_parts(pgid, probe.clone(), Duration::ZERO);
        // No owned child in the offline constructor: reap() just marks reaped.
        let reaped = guard.reap().expect("reap marks the guard reaped");
        assert!(reaped.is_none(), "offline guard owns no child to wait on");
    }

    assert!(
        probe.recorded().is_empty(),
        "a reaped guard must never re-signal a (possibly recycled) pgid (REQ-V2)"
    );
}

// ---------------------------------------------------------------------------
// LibcSignaller — the production seam's fail-closed guard.
// ---------------------------------------------------------------------------

/// The production signaller MUST reject a non-positive pgid with an error and
/// issue no `kill`. This is a load-bearing safety guard: calling it here is
/// safe precisely because the implementation short-circuits BEFORE any
/// `libc::kill`.
#[test]
fn libc_signaller_rejects_nonpositive_pgid() {
    let signaller = LibcSignaller;
    for pgid in [0, 1, -1, i32::MIN] {
        assert!(
            signaller.signal_group(pgid, sigkill()).is_err(),
            "LibcSignaller must refuse to signal non-positive pgid={pgid} (REQ-V1)"
        );
    }
}

/// `group_alive` MUST answer `false` for a non-positive pgid without probing —
/// never inspect the caller's own group or broadcast.
#[test]
fn libc_signaller_group_alive_false_for_nonpositive_pgid() {
    let signaller = LibcSignaller;
    for pgid in [0, 1, -1, i32::MIN] {
        assert!(
            !signaller.group_alive(pgid),
            "group_alive must be false for non-positive pgid={pgid}"
        );
    }
}
