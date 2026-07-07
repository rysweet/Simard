//! Hermetic behavior tests for the structured failure-diagnosis classifier
//! ([`crate::overseer::diagnosis`]).
//!
//! This module was previously exercised only *indirectly* (via the two live
//! call sites in `terminal_session::execution` and `journal::recipe`), leaving
//! most of its branch logic — the exit-code/marker precedence ordering, the
//! spawn-errno mapping, and both evidence-bounding paths — unverified. These
//! tests drive the public API directly and assert on concrete classified
//! causes, stable serialised labels, and exact evidence-bound sizes.
//!
//! Every test is hermetic: `ExitStatus` values are synthesised with
//! `ExitStatusExt::from_raw` (the same pattern used by
//! `terminal_session::execution`'s tests) and `io::Error` values are built with
//! `from_raw_os_error` / `io::Error::new` — no process is ever spawned and no
//! filesystem/network is touched.

use std::io;

use super::diagnosis::{
    FailureCause, MAX_EVIDENCE_LEN, classify_spawn_failure, classify_terminal_failure,
};

/// Synthesise a real `ExitStatus` carrying `code` as its exit code, without
/// spawning a process. Mirrors `terminal_session::execution`'s test helper.
#[cfg(unix)]
fn exit_status_with_code(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // On Unix the exit code lives in the high byte of the wait-status word.
    std::process::ExitStatus::from_raw(code << 8)
}

/// Synthesise a signal-terminated `ExitStatus` (no exit code) without spawning.
#[cfg(unix)]
fn signal_terminated_status(signal: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // A raw wait status whose low 7 bits are non-zero is signal-terminated, so
    // `.code()` returns `None`.
    std::process::ExitStatus::from_raw(signal)
}

// ---------------------------------------------------------------------------
// FailureCause::as_str — stable, unique kebab-case labels for every variant.
// ---------------------------------------------------------------------------

#[test]
fn as_str_maps_every_variant_to_its_stable_label() {
    assert_eq!(FailureCause::ArgListTooLong.as_str(), "arg-list-too-long");
    assert_eq!(FailureCause::CommandNotFound.as_str(), "command-not-found");
    assert_eq!(FailureCause::PermissionDenied.as_str(), "permission-denied");
    assert_eq!(FailureCause::DiskFull.as_str(), "disk-full");
    assert_eq!(FailureCause::OutOfMemory.as_str(), "out-of-memory");
    assert_eq!(FailureCause::NetworkOrAuth.as_str(), "network-or-auth");
    assert_eq!(FailureCause::Unknown.as_str(), "unknown");
}

#[test]
fn as_str_labels_are_all_distinct() {
    let all = [
        FailureCause::ArgListTooLong,
        FailureCause::CommandNotFound,
        FailureCause::PermissionDenied,
        FailureCause::DiskFull,
        FailureCause::OutOfMemory,
        FailureCause::NetworkOrAuth,
        FailureCause::Unknown,
    ];
    let mut labels: Vec<&str> = all.iter().map(|c| c.as_str()).collect();
    let total = labels.len();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        total,
        "two FailureCause variants share a label"
    );
}

#[test]
fn display_matches_as_str_for_every_variant() {
    for cause in [
        FailureCause::ArgListTooLong,
        FailureCause::CommandNotFound,
        FailureCause::PermissionDenied,
        FailureCause::DiskFull,
        FailureCause::OutOfMemory,
        FailureCause::NetworkOrAuth,
        FailureCause::Unknown,
    ] {
        assert_eq!(format!("{cause}"), cause.as_str());
    }
}

#[test]
fn failure_cause_serialises_as_its_kebab_label() {
    // Serialize is a bare string, never a numeric tag or struct.
    assert_eq!(
        serde_json::to_string(&FailureCause::ArgListTooLong).unwrap(),
        "\"arg-list-too-long\""
    );
    assert_eq!(
        serde_json::to_string(&FailureCause::NetworkOrAuth).unwrap(),
        "\"network-or-auth\""
    );
    assert_eq!(
        serde_json::to_string(&FailureCause::Unknown).unwrap(),
        "\"unknown\""
    );
}

// ---------------------------------------------------------------------------
// classify_terminal_failure — transcript markers first, then exit-code hints.
// ---------------------------------------------------------------------------

/// Helper: classify a terminal failure with the given exit code + transcript
/// and return just the cause.
#[cfg(unix)]
fn cause_of(code: i32, transcript: &str) -> FailureCause {
    classify_terminal_failure(&exit_status_with_code(code), transcript).cause
}

#[cfg(unix)]
#[test]
fn arg_list_marker_wins_over_exit_126_not_executable() {
    // The headline live defect: exit 126 alone would read "permission denied",
    // but the E2BIG marker must take precedence.
    assert_eq!(
        cause_of(126, "execve: Argument list too long"),
        FailureCause::ArgListTooLong
    );
    // The bare `e2big` token is honoured too.
    assert_eq!(
        cause_of(126, "posix_spawn failed: E2BIG"),
        FailureCause::ArgListTooLong
    );
}

#[cfg(unix)]
#[test]
fn marker_matching_is_case_insensitive() {
    assert_eq!(
        cause_of(1, "ARGUMENT LIST TOO LONG"),
        FailureCause::ArgListTooLong
    );
    assert_eq!(
        cause_of(1, "No Space Left On Device"),
        FailureCause::DiskFull
    );
    assert_eq!(
        cause_of(1, "Connection Refused"),
        FailureCause::NetworkOrAuth
    );
}

#[cfg(unix)]
#[test]
fn disk_full_markers_classify_as_disk_full() {
    assert_eq!(
        cause_of(1, "write: no space left on device"),
        FailureCause::DiskFull
    );
    assert_eq!(cause_of(1, "ENOSPC while flushing"), FailureCause::DiskFull);
    // A disk-full marker must win over the exit-127 "command not found" hint.
    assert_eq!(
        cause_of(127, "no space left on device"),
        FailureCause::DiskFull
    );
}

#[cfg(unix)]
#[test]
fn out_of_memory_markers_classify_as_oom() {
    assert_eq!(
        cause_of(1, "fatal: out of memory"),
        FailureCause::OutOfMemory
    );
    assert_eq!(cause_of(1, "oom-kill triggered"), FailureCause::OutOfMemory);
    assert_eq!(cause_of(1, "invoked oom killer"), FailureCause::OutOfMemory);
    assert_eq!(
        cause_of(1, "Killed process 1234 (cargo)"),
        FailureCause::OutOfMemory
    );
}

#[cfg(unix)]
#[test]
fn network_and_auth_markers_classify_as_network_or_auth() {
    for marker in [
        "could not resolve host: github.com",
        "temporary failure in name resolution",
        "connection refused",
        "network is unreachable",
        "no route to host",
        "connection timed out",
        "authentication failed for 'https://...'",
        "fatal: could not read Username for 'https://github.com'",
    ] {
        assert_eq!(
            cause_of(1, marker),
            FailureCause::NetworkOrAuth,
            "marker did not classify as NetworkOrAuth: {marker:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn command_not_found_from_marker_or_exit_127() {
    assert_eq!(
        cause_of(1, "sh: say: command not found"),
        FailureCause::CommandNotFound
    );
    // Canonical exit 127 with no marker still classifies.
    assert_eq!(cause_of(127, ""), FailureCause::CommandNotFound);
}

#[cfg(unix)]
#[test]
fn permission_denied_from_marker_or_exit_126() {
    assert_eq!(
        cause_of(1, "bash: ./x.sh: Permission denied"),
        FailureCause::PermissionDenied
    );
    // Canonical exit 126 with no marker.
    assert_eq!(cause_of(126, ""), FailureCause::PermissionDenied);
}

#[cfg(unix)]
#[test]
fn exit_137_without_marker_is_last_resort_oom() {
    assert_eq!(cause_of(137, ""), FailureCause::OutOfMemory);
}

#[cfg(unix)]
#[test]
fn command_not_found_marker_wins_over_exit_137_oom_hint() {
    // A textual marker (checked before the 137 code hint) must win: a shell
    // that printed "command not found" but happened to exit 137 is not OOM.
    assert_eq!(
        cause_of(137, "command not found"),
        FailureCause::CommandNotFound
    );
}

#[cfg(unix)]
#[test]
fn unrecognised_failure_is_structured_unknown_never_a_silent_drop() {
    assert_eq!(
        cause_of(1, "some unremarkable failure"),
        FailureCause::Unknown
    );
    assert_eq!(cause_of(2, ""), FailureCause::Unknown);
}

#[cfg(unix)]
#[test]
fn classify_terminal_failure_carries_exit_code_and_evidence() {
    let d = classify_terminal_failure(&exit_status_with_code(42), "  boom   happened \n");
    assert_eq!(d.cause, FailureCause::Unknown);
    assert_eq!(d.exit_code, Some(42));
    // Evidence is whitespace-collapsed into a single line.
    assert_eq!(d.evidence, "boom happened");
}

#[cfg(unix)]
#[test]
fn signal_terminated_status_has_no_exit_code_but_still_classifies() {
    // SIGKILL-terminated: no exit code, but a transcript marker still drives a
    // structured cause.
    let d = classify_terminal_failure(&signal_terminated_status(9), "out of memory");
    assert_eq!(
        d.exit_code, None,
        "signal death must yield exit_code = None"
    );
    assert_eq!(d.cause, FailureCause::OutOfMemory);

    // No marker + no exit code => structured Unknown (never a silent drop).
    let d2 = classify_terminal_failure(&signal_terminated_status(9), "worker vanished");
    assert_eq!(d2.exit_code, None);
    assert_eq!(d2.cause, FailureCause::Unknown);
}

// ---------------------------------------------------------------------------
// classify_spawn_failure — errno first, then a message-string fallback.
// ---------------------------------------------------------------------------

#[test]
fn spawn_errno_e2big_maps_to_arg_list_too_long() {
    let d = classify_spawn_failure(&io::Error::from_raw_os_error(7));
    assert_eq!(d.cause, FailureCause::ArgListTooLong);
    // A pre-exec spawn failure never has a child, so exit_code is always None.
    assert_eq!(d.exit_code, None);
    // Evidence is the (whitespace-collapsed) OS message.
    assert!(
        d.evidence.contains("os error 7"),
        "evidence: {:?}",
        d.evidence
    );
}

#[test]
fn spawn_errno_enospc_maps_to_disk_full() {
    let d = classify_spawn_failure(&io::Error::from_raw_os_error(28));
    assert_eq!(d.cause, FailureCause::DiskFull);
    assert_eq!(d.exit_code, None);
}

#[test]
fn spawn_errno_enomem_maps_to_out_of_memory() {
    let d = classify_spawn_failure(&io::Error::from_raw_os_error(12));
    assert_eq!(d.cause, FailureCause::OutOfMemory);
}

#[test]
fn spawn_unmapped_errno_is_structured_unknown() {
    // ENOENT (2) is not one of the mapped spawn causes and its OS message
    // carries no arg-list marker, so it classifies as Unknown — structurally
    // recorded, never dropped.
    let d = classify_spawn_failure(&io::Error::from_raw_os_error(2));
    assert_eq!(d.cause, FailureCause::Unknown);
    assert_eq!(d.exit_code, None);
}

#[test]
fn spawn_message_fallback_catches_arg_list_when_no_errno_present() {
    // An error with no numeric errno but an arg-list message still classifies
    // via the string fallback.
    let d = classify_spawn_failure(&io::Error::other("posix_spawn: Argument list too long"));
    assert!(
        io::Error::other("x").raw_os_error().is_none(),
        "sanity: io::Error::other carries no os errno"
    );
    assert_eq!(d.cause, FailureCause::ArgListTooLong);
    assert_eq!(d.exit_code, None);
}

#[test]
fn spawn_message_fallback_catches_e2big_token() {
    let d = classify_spawn_failure(&io::Error::other("spawn failed: E2BIG"));
    assert_eq!(d.cause, FailureCause::ArgListTooLong);
}

#[test]
fn spawn_message_without_marker_and_no_errno_is_unknown() {
    let d = classify_spawn_failure(&io::Error::other("totally unrelated"));
    assert_eq!(d.cause, FailureCause::Unknown);
    assert_eq!(d.evidence, "totally unrelated");
}

// ---------------------------------------------------------------------------
// Evidence bounding — bounded_evidence (terminal) keeps the TAIL with a
// leading ellipsis; bounded_spawn_evidence (spawn) keeps the HEAD with a
// trailing ellipsis and caps the WHOLE string at MAX_EVIDENCE_LEN.
// ---------------------------------------------------------------------------

#[test]
fn max_evidence_len_is_the_documented_bound() {
    assert_eq!(MAX_EVIDENCE_LEN, 400);
}

#[cfg(unix)]
#[test]
fn terminal_evidence_collapses_whitespace_and_passes_short_input_through() {
    let d = classify_terminal_failure(&exit_status_with_code(1), "a  b\tc\n\nd   e");
    assert_eq!(d.evidence, "a b c d e");
}

#[cfg(unix)]
#[test]
fn terminal_evidence_at_exact_bound_is_returned_verbatim() {
    let line = "x".repeat(MAX_EVIDENCE_LEN); // no whitespace => one token
    let d = classify_terminal_failure(&exit_status_with_code(1), &line);
    assert_eq!(d.evidence.chars().count(), MAX_EVIDENCE_LEN);
    assert_eq!(
        d.evidence, line,
        "input exactly at the bound must be verbatim"
    );
    assert!(!d.evidence.starts_with('…'));
}

#[cfg(unix)]
#[test]
fn terminal_evidence_over_bound_keeps_tail_with_leading_ellipsis() {
    // Distinct head vs tail so we can prove the TAIL is retained.
    let input = format!("HEAD{}TAILEND", "m".repeat(500));
    let d = classify_terminal_failure(&exit_status_with_code(1), &input);
    let ev = d.evidence;
    assert!(ev.starts_with('…'), "expected leading ellipsis: {ev:?}");
    // Ellipsis + exactly MAX_EVIDENCE_LEN retained chars.
    assert_eq!(ev.chars().count(), MAX_EVIDENCE_LEN + 1);
    // The retained portion is the true tail of the collapsed input.
    let retained: String = ev.chars().skip(1).collect();
    let expected_tail: String = {
        let chars: Vec<char> = input.chars().collect();
        chars[chars.len() - MAX_EVIDENCE_LEN..].iter().collect()
    };
    assert_eq!(retained, expected_tail);
    assert!(ev.ends_with("TAILEND"), "tail content must survive: {ev:?}");
    assert!(!ev.contains("HEAD"), "head must be dropped: {ev:?}");
}

#[test]
fn spawn_evidence_collapses_whitespace_for_short_messages() {
    let d = classify_spawn_failure(&io::Error::other("boom   \n  bang"));
    assert_eq!(d.evidence, "boom bang");
}

#[test]
fn spawn_evidence_at_exact_bound_is_returned_verbatim() {
    let msg = "y".repeat(MAX_EVIDENCE_LEN);
    let d = classify_spawn_failure(&io::Error::other(msg.clone()));
    assert_eq!(d.evidence.chars().count(), MAX_EVIDENCE_LEN);
    assert_eq!(d.evidence, msg);
    assert!(!d.evidence.ends_with('…'));
}

#[test]
fn spawn_evidence_over_bound_keeps_head_and_caps_total_at_bound() {
    // Head-preserving with a trailing ellipsis; unlike the terminal path, the
    // WHOLE string (ellipsis included) is capped at MAX_EVIDENCE_LEN.
    let msg = format!("STARTHEAD{}", "z".repeat(500));
    let d = classify_spawn_failure(&io::Error::other(msg.clone()));
    let ev = d.evidence;
    assert!(ev.ends_with('…'), "expected trailing ellipsis: {ev:?}");
    assert_eq!(
        ev.chars().count(),
        MAX_EVIDENCE_LEN,
        "spawn evidence must cap the whole string at the bound"
    );
    // The retained portion is the true head of the collapsed message.
    let head: String = ev.chars().take(MAX_EVIDENCE_LEN - 1).collect();
    let expected_head: String = msg.chars().take(MAX_EVIDENCE_LEN - 1).collect();
    assert_eq!(head, expected_head);
    assert!(
        ev.starts_with("STARTHEAD"),
        "head content must survive: {ev:?}"
    );
}

// ---------------------------------------------------------------------------
// FailureDiagnosis — value semantics and full JSON shape.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn diagnosis_equality_and_clone_are_value_based() {
    let a = classify_terminal_failure(&exit_status_with_code(127), "command not found");
    let b = a.clone();
    assert_eq!(a, b);

    let c = classify_terminal_failure(&exit_status_with_code(126), "permission denied");
    assert_ne!(a, c, "different cause + exit code must not compare equal");
}

#[cfg(unix)]
#[test]
fn diagnosis_serialises_cause_as_string_with_exit_code_and_evidence() {
    let d = classify_terminal_failure(&exit_status_with_code(127), "sh: nope: command not found");
    let v: serde_json::Value = serde_json::to_value(&d).unwrap();
    assert_eq!(v["cause"], serde_json::json!("command-not-found"));
    assert_eq!(v["exit_code"], serde_json::json!(127));
    assert_eq!(
        v["evidence"],
        serde_json::json!("sh: nope: command not found")
    );
}

#[test]
fn spawn_diagnosis_serialises_exit_code_as_null() {
    let d = classify_spawn_failure(&io::Error::from_raw_os_error(7));
    let v: serde_json::Value = serde_json::to_value(&d).unwrap();
    assert_eq!(v["cause"], serde_json::json!("arg-list-too-long"));
    assert!(v["exit_code"].is_null(), "spawn failure has no exit code");
}
