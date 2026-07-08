//! Guarded external-service command seam — the single execution chokepoint that
//! enforces the observe-only (read-only) floor for **external services**
//! (`gh`, `az`, `curl`, `wget`, `az rest`).
//!
//! ## Why this module (Step 8b: external-service integration)
//!
//! [`crate::read_only_guard`] is the *classifier*: given an argument vector it
//! decides whether a command is a provable read or a (possibly) mutating write.
//! [`crate::git_guardrails::check_git_safety`] already wires that classifier
//! into the shared **git** write seam. But Simard also shells out to the GitHub
//! CLI (`gh`) to create issues, merge PRs, and comment — none of which flow
//! through the git seam. Under the Crocutus (observe-only) identity those
//! external-service writes must be refused too.
//!
//! Rather than duplicate the env-gate/classify/spawn dance at every call site,
//! this module provides one small seam:
//!
//! - [`run_output`] / [`run_status`] — screen the argv, then spawn (the drop-in
//!   replacement for `Command::new(prog).args(a).output()/.status()`).
//! - [`screen`] — screen an already-assembled argv without spawning, for sites
//!   that build a [`std::process::Command`] incrementally (e.g. with
//!   `current_dir`/`env`) or use an async runtime.
//!
//! ## Fail-closed semantics
//!
//! When the observe-only env flag ([`crate::read_only_guard::OBSERVE_ONLY_ENV`])
//! is set and the command is not a provable read, the seam returns
//! `Err(io::Error)` of kind [`std::io::ErrorKind::PermissionDenied`] carrying the
//! stable `GUARDRAIL BLOCKED` message — and **never spawns the process**. When
//! the flag is unset (the engineer identity, Simard) the seam is a transparent
//! pass-through: it behaves exactly like the original `Command` call, so the
//! `io::Result` error surface every call site already handles is preserved.

use std::ffi::OsStr;
use std::io;
use std::process::{Command, ExitStatus, Output};

/// Screen a full command line (program **plus** its arguments) against the
/// observe-only floor without spawning anything.
///
/// `argv[0]` is the program (`"gh"`, `"az"`, `"curl"`, …) and the remainder are
/// its arguments. Returns `Ok(())` when the command is a provable read or the
/// observe-only flag is unset; returns an [`io::ErrorKind::PermissionDenied`]
/// error carrying the `GUARDRAIL BLOCKED` message otherwise.
///
/// Use this for call sites that assemble a [`Command`] incrementally (with
/// `current_dir`, `env`, argument builders) or run on an async executor: call
/// `screen` first, and only spawn if it returns `Ok`.
pub fn screen<S: AsRef<str>>(argv: &[S]) -> io::Result<()> {
    let refs: Vec<&str> = argv.iter().map(|s| s.as_ref()).collect();
    crate::read_only_guard::guard_observe_only(&refs).map_err(blocked_io_error)
}

/// Screen `[program, args…]` and, if permitted, run it to completion capturing
/// its [`Output`].
///
/// Drop-in for `Command::new(program).args(args).output()`: the returned
/// `io::Result<Output>` is identical in the allowed case, and in the blocked
/// case is `Err` of kind [`io::ErrorKind::PermissionDenied`] with the guardrail
/// message — the process is never spawned.
pub fn run_output<S: AsRef<str>>(program: &str, args: &[S]) -> io::Result<Output> {
    screen_program(program, args)?;
    Command::new(program).args(as_os_str_iter(args)).output()
}

/// Screen `[program, args…]` and, if permitted, run it capturing only its
/// [`ExitStatus`].
///
/// Drop-in for `Command::new(program).args(args).status()`, with the same
/// fail-closed semantics as [`run_output`].
pub fn run_status<S: AsRef<str>>(program: &str, args: &[S]) -> io::Result<ExitStatus> {
    screen_program(program, args)?;
    Command::new(program).args(as_os_str_iter(args)).status()
}

/// Screen a `program` + separate `args` slice (the seam's internal form).
fn screen_program<S: AsRef<str>>(program: &str, args: &[S]) -> io::Result<()> {
    let mut refs: Vec<&str> = Vec::with_capacity(args.len() + 1);
    refs.push(program);
    refs.extend(args.iter().map(|s| s.as_ref()));
    crate::read_only_guard::guard_observe_only(&refs).map_err(blocked_io_error)
}

/// Borrow each arg as an [`OsStr`] for [`Command::args`] without allocating.
fn as_os_str_iter<S: AsRef<str>>(args: &[S]) -> impl Iterator<Item = &OsStr> {
    args.iter().map(|s| OsStr::new(s.as_ref()))
}

/// Wrap a guardrail block message as a `PermissionDenied` I/O error so blocked
/// commands surface through the same `io::Result` path callers already handle.
fn blocked_io_error(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_only_guard::OBSERVE_ONLY_ENV;
    use std::sync::Mutex;

    // These tests mutate the process-global OBSERVE_ONLY_ENV var; serialize
    // them against each other and against the other observe-only env tests
    // (same `cognitive_memory` serial key used across the crate).
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
    fn observe_only_blocks_gh_writes_at_the_seam() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_observe(true);
        for args in [
            vec!["issue", "create", "--title", "x", "--body", "y"],
            vec!["pr", "merge", "1", "--merge"],
            vec!["pr", "comment", "1", "--body", "hi"],
            vec!["issue", "close", "1"],
        ] {
            let err = run_output("gh", &args).expect_err("write must be refused");
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert!(
                err.to_string().contains("GUARDRAIL BLOCKED"),
                "blocked error must carry the stable marker, got: {err}"
            );
        }
        set_observe(false);
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn observe_only_allows_reads_through_screen() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_observe(true);
        // Reads must pass so the observer can still see the target repos.
        assert!(screen(&["gh", "issue", "list", "--state", "open"]).is_ok());
        assert!(screen(&["gh", "pr", "view", "1"]).is_ok());
        assert!(screen(&["az", "repos", "pr", "list"]).is_ok());
        assert!(screen(&["curl", "https://example.test/x"]).is_ok());
        // …and writes are still refused via `screen`.
        assert!(screen(&["gh", "issue", "create", "--title", "x"]).is_err());
        assert!(screen(&["az", "repos", "pr", "create"]).is_err());
        assert!(screen(&["curl", "-X", "POST", "-d", "b", "https://example.test/x"]).is_err());
        set_observe(false);
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn engineer_identity_is_a_transparent_passthrough() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_observe(false);
        // With the flag unset, even a mutating gh command screens Ok (the
        // engineer identity is free to write); the seam does not spawn here.
        assert!(screen(&["gh", "issue", "create", "--title", "x"]).is_ok());
        assert!(screen(&["gh", "pr", "merge", "1", "--merge"]).is_ok());
    }

    #[test]
    fn seam_spawns_and_captures_output_when_allowed() {
        // `echo` is out of the screened tool set, so it is permitted regardless
        // of identity — this proves the seam actually spawns and returns Output.
        let out = run_output("echo", &["hello-seam"]).expect("echo must run");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello-seam");
    }

    #[test]
    fn accepts_owned_string_args() {
        // The routing client passes `&[String]`; the generic bound must accept
        // owned strings as well as &str.
        let args: Vec<String> = vec!["hi-owned".to_string()];
        let out = run_output("echo", &args).expect("echo must run");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi-owned");
    }
}
