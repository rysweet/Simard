//! Self-diagnose-on-step-error contract (issue #2640, PART 2).
//!
//! Operator ask: when a decision-cycle / engineer / terminal-shell step fails,
//! Simard must NOT just LOG the error and move on. She must INSPECT and
//! DIAGNOSE *why* it happened — classify the failure and record a structured
//! root cause — then drive a CORRECTIVE response the OODA/Overseer loop acts on
//! (an actionable Signal / intervention), not a silent log line.
//!
//! This pins that contract end-to-end through the PUBLIC surface:
//!
//!   raw step failure  ─▶  classify_terminal_failure  ─▶  FailureDiagnosis
//!                       ─▶  record_step_failure (bounded sink)
//!                       ─▶  ObservedState.recent_step_failures
//!                       ─▶  signals_from  ─▶  Signal::StepFailureDiagnosed
//!                       ─▶  orient  ─▶  Problem
//!                       ─▶  decide  ─▶  a corrective Intervention (NOT a log)
//!
//! TDD status: RED until the fix adds the diagnosis types, the failure sink,
//! the `Signal::StepFailureDiagnosed` variant, the `ObservedState`
//! `recent_step_failures` field, and their wiring. Isolated integration crate,
//! so the red compile does not affect the rest of the suite.
//!
//! Gated `#[cfg(unix)]` for `ExitStatusExt::from_raw` (mirrors the existing
//! terminal-failure tests in `src/terminal_session/execution.rs`).

#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use serial_test::serial;

use simard::overseer::failure_sink::{
    STEP_FAILURE_SINK_CAPACITY, drain_recent, record_step_failure,
};
use simard::overseer::signal::Signal;
use simard::overseer::{
    FailureCause, FailureDiagnosis, Intervention, ObservedState, classify_terminal_failure, decide,
    orient, signals_from,
};

/// Build an `ExitStatus` carrying `code` (encoded as the wait-status high byte),
/// matching the helper used by `src/terminal_session/execution.rs` tests.
fn exit_with_code(code: i32) -> ExitStatus {
    ExitStatus::from_raw(code << 8)
}

// ---------------------------------------------------------------------------
// classify_terminal_failure — structured root-cause classification
// ---------------------------------------------------------------------------

/// The headline live defect: exit 126 whose transcript carries the kernel's
/// "Argument list too long" must classify as `ArgListTooLong` — the exact
/// E2BIG failure PART 1 fixes. This is the case the operator saw repeatedly.
#[test]
fn classify_exit_126_argument_list_too_long_is_arg_list_too_long() {
    let transcript = "base type 'decision cycle-copilot' failed ... \
        bash: /home/azureuser/.local/bin/amplihack: Argument list too long";
    let diag = classify_terminal_failure(&exit_with_code(126), transcript);

    assert_eq!(
        diag.cause,
        FailureCause::ArgListTooLong,
        "126 + 'Argument list too long' must diagnose as arg-list-too-long, \
         not a bare 'not executable': {diag:?}"
    );
    assert_eq!(
        diag.cause.as_str(),
        "arg-list-too-long",
        "the stable cause label must be `arg-list-too-long` (per the issue)"
    );
    assert_eq!(diag.exit_code, Some(126), "the exit code must be recorded");
}

/// A bare exit 127 is a missing command on PATH.
#[test]
fn classify_exit_127_is_command_not_found() {
    let diag =
        classify_terminal_failure(&exit_with_code(127), "bash: amplihack: command not found");
    assert_eq!(diag.cause, FailureCause::CommandNotFound, "{diag:?}");
}

/// Exit 126 with a "Permission denied" transcript (and no arg-list marker) is a
/// permission failure, distinct from arg-list-too-long.
#[test]
fn classify_exit_126_permission_denied_is_permission_denied() {
    let diag = classify_terminal_failure(&exit_with_code(126), "bash: ./run.sh: Permission denied");
    assert_eq!(diag.cause, FailureCause::PermissionDenied, "{diag:?}");
}

/// ENOSPC surfaced in the transcript classifies as disk-full.
#[test]
fn classify_no_space_left_is_disk_full() {
    let diag = classify_terminal_failure(
        &exit_with_code(1),
        "fatal: write error: No space left on device",
    );
    assert_eq!(diag.cause, FailureCause::DiskFull, "{diag:?}");
}

/// An OOM-kill surfaced in the transcript classifies as out-of-memory.
#[test]
fn classify_out_of_memory_is_out_of_memory() {
    let diag = classify_terminal_failure(
        &exit_with_code(137),
        "Out of memory: Killed process 12345 (amplihack)",
    );
    assert_eq!(diag.cause, FailureCause::OutOfMemory, "{diag:?}");
}

/// Network/DNS/auth failures in the transcript classify as network-or-auth.
#[test]
fn classify_network_failure_is_network_or_auth() {
    let diag = classify_terminal_failure(
        &exit_with_code(6),
        "curl: (6) Could not resolve host: api.github.com",
    );
    assert_eq!(diag.cause, FailureCause::NetworkOrAuth, "{diag:?}");
}

/// An unrecognised failure must degrade to `Unknown` (never a panic, never a
/// silent drop) so the caller still records *something* structured.
#[test]
fn classify_unrecognised_failure_is_unknown() {
    let diag = classify_terminal_failure(&exit_with_code(3), "widget frobnicator returned 3");
    assert_eq!(diag.cause, FailureCause::Unknown, "{diag:?}");
}

// ---------------------------------------------------------------------------
// FailureDiagnosis — structured, serialisable record (not a log line)
// ---------------------------------------------------------------------------

/// The diagnosis is a structured value, not a formatted log string, and it is
/// serialisable so it can travel on the Overseer activity feed / structured log.
#[test]
fn failure_diagnosis_is_serialisable_with_cause_label() {
    let diag = classify_terminal_failure(&exit_with_code(126), "amplihack: Argument list too long");
    let json = serde_json::to_string(&diag).expect("FailureDiagnosis must be Serialize");
    assert!(
        json.contains("arg-list-too-long"),
        "serialised diagnosis must carry the stable cause label: {json}"
    );
}

// ---------------------------------------------------------------------------
// failure_sink — bounded ring buffer (memory-DoS safe)
// ---------------------------------------------------------------------------

/// Recorded diagnoses are retained in a bounded ring buffer and returned by
/// `drain_recent`; overflow evicts the OLDEST, keeping the most recent
/// `STEP_FAILURE_SINK_CAPACITY`. Serialised because the sink is process-global.
#[test]
#[serial(step_failure_sink)]
fn failure_sink_is_bounded_and_drains_most_recent() {
    // Clear any residue from earlier tests.
    let _ = drain_recent();

    let overflow = STEP_FAILURE_SINK_CAPACITY + 3;
    for i in 0..overflow {
        record_step_failure(FailureDiagnosis {
            cause: FailureCause::Unknown,
            exit_code: Some(1),
            evidence: format!("marker-{i}"),
        });
    }

    let drained = drain_recent();
    assert_eq!(
        drained.len(),
        STEP_FAILURE_SINK_CAPACITY,
        "the sink must be bounded to STEP_FAILURE_SINK_CAPACITY entries"
    );
    // The retained entries must be the most recent ones (oldest evicted).
    let first_kept = &drained.first().expect("non-empty").evidence;
    assert_eq!(
        first_kept,
        &format!("marker-{}", overflow - STEP_FAILURE_SINK_CAPACITY),
        "the oldest entries must be evicted, keeping the most recent: {drained:?}"
    );
    // Draining empties the sink.
    assert!(
        drain_recent().is_empty(),
        "drain_recent must consume the buffer"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: diagnosis drives a CORRECTIVE action, not a silent log
// ---------------------------------------------------------------------------

/// `signals_from` must lift a recorded `FailureDiagnosis` into a
/// `Signal::StepFailureDiagnosed` so the Overseer's Orient/Decide loop can act.
#[test]
fn observed_step_failure_becomes_a_signal() {
    let diag = classify_terminal_failure(
        &exit_with_code(126),
        "bash: amplihack: Argument list too long",
    );
    let state = ObservedState {
        recent_step_failures: vec![diag],
        ..ObservedState::default()
    };

    let signals = signals_from(&state);
    let found = signals.iter().any(|s| {
        matches!(
            s,
            Signal::StepFailureDiagnosed {
                cause: FailureCause::ArgListTooLong,
                ..
            }
        )
    });
    assert!(
        found,
        "a recorded arg-list-too-long step failure must surface as a \
         Signal::StepFailureDiagnosed: {signals:?}"
    );
}

/// The full seam: an E2BIG step failure must resolve to a CORRECTIVE
/// intervention the loop acts on — NOT a no-op `Report` and NOT a silent log.
/// This is the crux of PART 2: diagnose the WHY, then drive a fix.
#[test]
fn diagnosed_step_failure_drives_corrective_intervention() {
    let diag = classify_terminal_failure(
        &exit_with_code(126),
        "terminal-shell session exited with status exit status: 126 ... \
         bash: /home/azureuser/.local/bin/amplihack: Argument list too long",
    );
    let state = ObservedState {
        recent_step_failures: vec![diag],
        ..ObservedState::default()
    };

    let signals = signals_from(&state);
    let problems = orient(&signals, &[]);
    let problem = problems
        .iter()
        .find(|p| {
            p.evidence
                .iter()
                .any(|s| matches!(s, Signal::StepFailureDiagnosed { .. }))
        })
        .expect("the diagnosed step failure must produce a Problem to act on");

    let intervention = decide(problem);

    // It must be a concrete corrective action, not a passive report.
    assert!(
        !matches!(intervention, Intervention::Report),
        "a diagnosed step failure must not resolve to a passive Report / log: \
         {intervention:?}"
    );

    match intervention {
        Intervention::LaunchRecipe { brief } => {
            let task = brief.task_description.to_ascii_lowercase();
            assert!(
                !task.is_empty(),
                "the corrective recipe must carry a task description"
            );
            assert!(
                task.contains("arg") || task.contains("too long") || task.contains("diagnos"),
                "the corrective action must reference the diagnosed root cause \
                 so the fix targets the real problem: {:?}",
                brief.task_description
            );
        }
        other => panic!(
            "expected a corrective LaunchRecipe workstream for an E2BIG step \
             failure, got {other:?}"
        ),
    }
}
