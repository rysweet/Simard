//! Argv-free invocation contract for Simard's OODA decision-cycle / engineer
//! invocation sites (issue #2640, PART 1).
//!
//! The E2BIG defect had THREE invocation sites that inlined the prompt into
//! argv via the `-p "$(cat <promptfile>)"` antipattern:
//!
//! * **Site A — meeting** (`base_type_copilot::run_meeting_turn`): a
//!   `sh -c "copilot ... -p \"$(cat 'PATH')\""` string.
//! * **Site B — engineering/decision-cycle** (`build_copilot_terminal_objective`):
//!   a PTY `command:` line (covered by in-crate unit tests in
//!   `src/tests_base_type_copilot.rs`).
//! * **Site C — OODA launch-session** (`ooda_actions` dispatch): a hand-escaped
//!   `printf '%s' '<task>' > $F && amplihack copilot -p "$(cat "$F")"` PTY
//!   command.
//!
//! This file pins the PUBLIC, testable builders the fix must expose for the two
//! sites NOT covered in-crate (A and C). Each builder must deliver the prompt
//! via STDIN / a `cat 'PATH' |` pipe so the prompt never contributes to argv.
//!
//! TDD status: RED until the fix exposes `build_meeting_command` and
//! `build_ooda_launch_command`. This is an isolated integration crate, so the
//! red compile does not affect the rest of the test suite.

#![cfg(unix)]

use std::ffi::OsStr;
use std::process::Command;

use simard::base_type_copilot::build_meeting_command;
use simard::ooda_actions::build_ooda_launch_command;

/// Collect a `std::process::Command`'s args as owned `String`s for assertions.
fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a: &OsStr| a.to_string_lossy().into_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Site A — meeting invocation
// ---------------------------------------------------------------------------

/// Site A must invoke the copilot binary DIRECTLY (no `sh -c` wrapper) and must
/// NOT pass the prompt via `-p`/argv. The prompt is streamed on stdin by the
/// caller via `prompt_delivery::apply_std(.., Stdin)` after spawn, so the
/// builder's argv is prompt-free and bounded — immune to E2BIG (issue #2640).
#[test]
fn meeting_command_is_direct_and_argv_free() {
    let session_id = "11111111-2222-3333-4444-555555555555";
    let cmd = build_meeting_command("copilot", session_id, Some("/tmp/work"));

    // Direct exec of the copilot binary — NOT `sh -c "..."`.
    let program = cmd.get_program().to_string_lossy().into_owned();
    assert_eq!(
        program, "copilot",
        "meeting turn must exec the copilot binary directly, not via a shell: \
         got program {program:?}"
    );

    let args = args_of(&cmd);

    // The non-interactive meeting flags must be present (issue #2170 parity).
    for flag in [
        "--no-custom-instructions",
        "--silent",
        "--allow-all-tools",
        "--session-id",
    ] {
        assert!(
            args.iter().any(|a| a == flag),
            "meeting command must include {flag:?}: {args:?}"
        );
    }
    assert!(
        args.iter().any(|a| a == session_id),
        "meeting command must carry the session id: {args:?}"
    );

    // Argv-free prompt delivery: NO `-p`, NO `$(cat ...)`, NO `sh`/`-c`.
    assert!(
        !args.iter().any(|a| a == "-p"),
        "issue #2640: meeting command must NOT pass the prompt via `-p`: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("$(cat")),
        "issue #2640: meeting command must NOT inline `$(cat ...)`: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "-c"),
        "meeting command must not be a `sh -c` wrapper: {args:?}"
    );
}

/// The builder must never embed the prompt body itself — regardless of the
/// prompt — because it is delivered on stdin, not argv.
#[test]
fn meeting_command_never_contains_prompt_body() {
    let cmd = build_meeting_command("copilot", "sess-abc", None);
    let program = cmd.get_program().to_string_lossy().into_owned();
    let args = args_of(&cmd);

    // A distinctive marker a real prompt might contain.
    let marker = "PROMPT-BODY-MARKER-2640";
    assert!(
        !program.contains(marker) && !args.iter().any(|a| a.contains(marker)),
        "the builder must not embed any prompt body in argv"
    );
    // And it must still be prompt-free of the argv antipatterns.
    assert!(!args.iter().any(|a| a == "-p"), "no -p flag: {args:?}");
}

// ---------------------------------------------------------------------------
// Site C — OODA launch-session invocation
// ---------------------------------------------------------------------------

/// Site C must build a PTY command that pipes the prompt file into copilot on
/// STDIN (`cat 'PATH' | amplihack copilot ... ; exit`) — NOT the old
/// hand-escaped `printf '%s' '<task>' | ... -p "$(cat "$F")"` form that inlined
/// the task into argv and blew past ARG_MAX (issue #2640).
#[test]
fn ooda_launch_command_pipes_prompt_via_stdin() {
    let path = std::path::Path::new("/tmp/simard-ooda-prompt.abc123");
    let command = build_ooda_launch_command("amplihack copilot", path)
        .expect("a clean temp path must produce a valid launch command");

    // Positive: `cat 'PATH' | amplihack copilot ...` with the copilot as sink.
    assert!(
        command.contains("cat '/tmp/simard-ooda-prompt.abc123' | amplihack copilot"),
        "launch command must pipe the prompt file into copilot on stdin: {command:?}"
    );
    assert!(
        command.contains("--subprocess-safe"),
        "launch command must keep --subprocess-safe: {command:?}"
    );
    assert!(
        command.contains("--allow-all-tools"),
        "launch command must keep --allow-all-tools: {command:?}"
    );
    assert!(
        command.contains("; exit"),
        "launch command must chain `; exit` so the PTY shell returns: {command:?}"
    );

    // Negative: none of the E2BIG / hand-escaping antipatterns.
    assert!(
        !command.contains("$(cat"),
        "issue #2640: no `$(cat ...)` argv expansion: {command:?}"
    );
    assert!(
        !command.contains(" -p "),
        "issue #2640: prompt must not be passed via `-p`: {command:?}"
    );
    assert!(
        !command.contains("printf"),
        "the hand-escaped `printf '%s' '<task>'` inliner must be gone: {command:?}"
    );
}

/// Fail-closed: a temp path containing a single quote could break out of the
/// single-quoted `cat 'PATH'` context. The builder must refuse it rather than
/// emit an injectable command (NO silent fallback — issue #2640 constraints).
#[test]
fn ooda_launch_command_rejects_single_quote_in_path() {
    let evil = std::path::Path::new("/tmp/pwn'; rm -rf ~ #");
    let result = build_ooda_launch_command("amplihack copilot", evil);
    assert!(
        result.is_err(),
        "a path containing a single quote must be rejected fail-closed, not \
         escaped or silently accepted: {result:?}"
    );
}
