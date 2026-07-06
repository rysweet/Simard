//! Hermetic proof of the E2BIG mechanism behind the journal recipe-spawn
//! failure (issues #2640/#2692), and of the fix's transport (the file channel),
//! using only the PUBLIC `simard::recipe_context_file::ContextFile` API plus
//! real `/bin/echo` / `/bin/cat` subprocesses.
//!
//! Simard's journal draft + de-jargon path repeatedly failed, once per hour,
//! with:
//!
//! ```text
//! WARN simard::journal: journal draft recipe failed; using the deterministic
//!   report drafter error=... recipe-runner-rs spawn failed: Argument list too
//!   long (os error 7)
//! ```
//!
//! ROOT CAUSE: `JournalRecipe::run` inlined the whole day's context into `argv`
//! via `cmd.arg("-c").arg(format!("day_context={value}"))`. When `value` (a full
//! 24 h of episodic memories / a whole narrative draft) exceeded the kernel's
//! `MAX_ARG_STRLEN` (128 KiB per argument on Linux), `execve` failed with
//! `E2BIG` (`errno 7`) BEFORE `recipe-runner-rs` ever started, and the journal
//! silently fell back to a raw-dump deterministic drafter.
//!
//! These tests assert the two halves of the fix's premise:
//!   1. **The bug is real** — a single argv token past the per-arg limit makes
//!      `exec` fail with `E2BIG` before the child runs.
//!   2. **The fix works** — the SAME oversized payload, delivered via the file
//!      channel (`ContextFile`, path on argv, content in a temp file), spawns a
//!      child that reads the full payload with NO size failure, because the
//!      payload no longer contributes to argv.
//!
//! Gated `#[cfg(unix)]`: relies on POSIX `exec` `E2BIG` semantics and the
//! `/bin/echo` + `/bin/cat` binaries. CI is Linux.
//!
//! TDD status: RED until the fix adds `simard::recipe_context_file::ContextFile`.
//! Isolated integration crate — the red compile does not affect the rest of the
//! suite (same convention as `tests/ooda_e2big_transport.rs`).

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

use simard::recipe_context_file::ContextFile;

/// A payload comfortably larger than Linux `MAX_ARG_STRLEN` (128 KiB) and larger
/// than the >256 KB threshold in the issue's verification note. This is the
/// realistic scale of a busy day's `day_context_json`.
const OVERSIZED_BYTES: usize = 1024 * 1024; // 1 MiB

fn oversized_day_context() -> String {
    // Shape it like the JSON the journal marshals, with a distinctive marker so
    // a leak into argv would be caught.
    let filler: String = "episodic-memory-content "
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect();
    format!("{{\"date\":\"2026-07-06\",\"episodes\":[\"{filler}\"]}}")
}

/// `E2BIG` is `7` on every Linux/Unix target Simard runs on; keep it named for
/// readability rather than pulling in the `libc` crate for one constant.
fn e2big() -> i32 {
    7
}

/// (1) THE BUG: passing an oversized `day_context` as a single `-c` argv token
/// makes `exec` fail with `E2BIG` (`errno 7`) before the child runs. This is
/// exactly the failure the old `JournalRecipe::run` hit at scale.
#[test]
fn oversized_day_context_as_argv_token_fails_with_e2big() {
    let payload = oversized_day_context();
    assert!(
        payload.len() > 256 * 1024,
        "payload must exceed the >256KB verification threshold"
    );

    // The old antipattern: the whole context inlined as one `-c KEY=VALUE` token.
    let inline_arg = format!("day_context={payload}");
    let result = Command::new("/bin/echo")
        .arg("-c")
        .arg(&inline_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    let err = result.expect_err(
        "exec with a >MAX_ARG_STRLEN argv token must fail — this is the E2BIG \
         condition that broke Simard's hourly journal (issues #2640/#2692)",
    );
    assert_eq!(
        err.raw_os_error(),
        Some(e2big()),
        "the argv-inlined oversized day_context must fail with E2BIG (errno 7), \
         got: {err:?}"
    );
}

/// (2) THE FIX: the SAME oversized `day_context`, delivered via the file channel
/// (`ContextFile`), spawns a child that reads the FULL payload from the file
/// with no size failure. Only the short `<key>_path=<abs>` sits on argv, so
/// ARG_MAX is irrelevant — `recipe-runner-rs` would spawn successfully.
#[test]
fn oversized_day_context_via_file_channel_spawns_successfully() {
    let payload = oversized_day_context();

    let cf = ContextFile::write("journal", "day_context", &payload)
        .expect("file-channel write must succeed for an oversized day_context");

    // Argv carries only the short path — never the payload.
    let arg = cf.arg_value();
    assert!(
        arg.len() < 4096 && !arg.contains("episodic-memory-content"),
        "the `-c` value must be a short path, not the inlined payload: {} bytes",
        arg.len()
    );

    // Spawn a real subprocess whose ONLY payload-bearing argv token is the file
    // PATH (`recipe-runner-rs <recipe> ... -c day_context_path=<abs>` reduces, at
    // the exec layer, to "a small path on argv + the child reads the file"). We
    // model the child's read with `/bin/cat <path>`.
    let output = Command::new("/bin/cat")
        .arg(cf.path())
        .stderr(Stdio::null())
        .output()
        .expect(
            "exec with only a short file PATH on argv must NOT fail with E2BIG — \
             this is the whole point of the file channel",
        );

    assert!(
        output.status.success(),
        "the child must read the context file cleanly, got status {}",
        output.status
    );
    assert_eq!(
        output.stdout.len(),
        payload.len(),
        "the file channel must deliver the full oversized payload byte-for-byte, \
         with no truncation"
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8"),
        payload,
        "round-tripped day_context must match exactly"
    );
}

/// (3) Streaming variant: even reading the oversized file on the child's STDIN
/// (`cat < file`) succeeds — proving the payload path never touches argv even
/// when the child consumes it as a stream.
#[test]
fn oversized_day_context_file_reads_on_stdin_without_e2big() {
    let payload = oversized_day_context();
    let cf = ContextFile::write("journal", "draft", &payload).expect("write");

    let file = std::fs::File::open(cf.path()).expect("open context file");
    let mut child = Command::new("/bin/cat")
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn /bin/cat with the context file on stdin");

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_end(&mut out)
        .expect("read stdout");
    assert!(child.wait().expect("wait").success());
    assert_eq!(out.len(), payload.len(), "full payload must stream through");
}
