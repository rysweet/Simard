//! Hermetic proof of the E2BIG mechanism behind issue #2640, and of the fix's
//! transport (stdin delivery), using only the PUBLIC `simard::prompt_delivery`
//! API plus real `/bin/echo` / `/bin/cat` subprocesses.
//!
//! Simard's OODA decision-cycle / engineer invocation path repeatedly failed
//! with:
//!
//! ```text
//! terminal-shell session exited with status exit status: 126 ...
//! bash: /home/azureuser/.local/bin/amplihack: Argument list too long
//! ```
//!
//! ROOT CAUSE: the invocation inlined the (large, accumulated-context) prompt
//! into argv via `amplihack copilot ... -p "$(cat <promptfile>)"`. When the
//! `$(cat ...)` expansion produced an argument string past the kernel's
//! `MAX_ARG_STRLEN` (128 KiB per argument on Linux), `exec` failed with
//! `E2BIG` (`errno 7`) and the shell surfaced it as exit 126.
//!
//! These tests assert the two halves of the fix's premise:
//!   1. **The bug is real** — a single argv token past the per-arg limit makes
//!      `exec` fail with `E2BIG` BEFORE the child ever runs.
//!   2. **The fix works** — the SAME oversized prompt, delivered on STDIN via
//!      [`simard::prompt_delivery::apply_std`], round-trips byte-for-byte with
//!      no size failure, because the prompt no longer contributes to argv.
//!
//! Gated `#[cfg(unix)]`: relies on POSIX `exec` `E2BIG` semantics and the
//! `/bin/echo` + `/bin/cat` binaries. CI is Linux.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

use simard::prompt_delivery::{PromptDelivery, apply_std};

/// A prompt comfortably larger than Linux `MAX_ARG_STRLEN` (128 KiB) and larger
/// than the >256 KB threshold called out in the issue's verification note.
const OVERSIZED_PROMPT_BYTES: usize = 1024 * 1024; // 1 MiB

fn oversized_prompt() -> Vec<u8> {
    // Mix in shell-significant bytes so the test would also have caught the
    // original quoting/escaping bugs, not just the size bug.
    b"author's \"merge\" `pwned` ${injected} objective:\n"
        .iter()
        .copied()
        .cycle()
        .take(OVERSIZED_PROMPT_BYTES)
        .collect()
}

/// (1) THE BUG: passing an oversized prompt as a single argv token makes
/// `exec` fail with `E2BIG` (`errno 7`) before the child runs. This is exactly
/// the failure the old `-p "$(cat ...)"` invocation hit at scale.
#[test]
fn oversized_prompt_as_argv_token_fails_with_e2big() {
    let prompt = oversized_prompt();
    assert!(
        prompt.len() > 256 * 1024,
        "prompt must exceed the >256KB verification threshold"
    );

    let arg = String::from_utf8(prompt).expect("ascii prompt");
    let result = Command::new("/bin/echo")
        .arg(&arg) // inline the whole prompt as ONE argv token — the antipattern
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    let err = result.expect_err(
        "exec with a >MAX_ARG_STRLEN argv token must fail — this is the E2BIG \
         condition that broke Simard's OODA loop (issue #2640)",
    );
    assert_eq!(
        err.raw_os_error(),
        Some(libc_e2big()),
        "the argv-inlined oversized prompt must fail with E2BIG (errno 7), \
         got: {err:?}"
    );
}

/// (2) THE FIX: the SAME oversized prompt, delivered on STDIN via
/// `apply_std(.., Stdin)`, round-trips byte-for-byte through `/bin/cat` with no
/// size failure. The prompt never touches argv, so ARG_MAX is irrelevant.
#[test]
fn oversized_prompt_via_stdin_succeeds() {
    let prompt = oversized_prompt();

    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let applied = apply_std(&mut cmd, &prompt, PromptDelivery::Stdin)
        .expect("stdin delivery must not fail for an oversized prompt");
    assert_eq!(
        applied.mode(),
        PromptDelivery::Stdin,
        "forced Stdin mode must be honoured (no silent fallback to Inline)"
    );

    let mut child = cmd.spawn().expect("spawn /bin/cat");
    // Feed the 1 MiB prompt on a separate thread so writing stdin cannot deadlock
    // against `/bin/cat` filling its (bounded) stdout pipe while we are still
    // writing — the same concurrent pattern the production meeting path uses.
    let stdin = child.stdin.take();
    let feeder = std::thread::spawn(move || applied.feed(stdin));

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_end(&mut out)
        .expect("read /bin/cat stdout");
    let status = child.wait().expect("wait /bin/cat");
    feeder
        .join()
        .expect("feeder thread must not panic")
        .expect("feeding the oversized prompt on stdin must succeed");

    assert!(status.success(), "/bin/cat should exit 0, got {status}");
    assert_eq!(
        out.len(),
        prompt.len(),
        "stdin transport must deliver the full prompt byte-for-byte"
    );
    assert_eq!(out, prompt, "round-tripped prompt bytes must match exactly");
}

/// (3) THE FIX, TempFile variant: for very large prompts the postmortem
/// temp-file transport (`TempFile` = 0o600 file + stdin) must also deliver the
/// full prompt without inlining it into argv.
#[test]
fn oversized_prompt_via_tempfile_succeeds() {
    let prompt = oversized_prompt();

    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let applied = apply_std(&mut cmd, &prompt, PromptDelivery::TempFile)
        .expect("temp-file delivery must not fail for an oversized prompt");
    assert_eq!(applied.mode(), PromptDelivery::TempFile);
    assert!(
        applied.temp_path().is_some(),
        "TempFile mode must expose the postmortem file path"
    );

    let mut child = cmd.spawn().expect("spawn /bin/cat");
    // Feed on a separate thread to avoid the stdin/stdout pipe deadlock (see the
    // Stdin test above).
    let stdin = child.stdin.take();
    let feeder = std::thread::spawn(move || applied.feed(stdin));

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut out)
        .expect("read stdout");
    assert!(child.wait().expect("wait").success());
    feeder
        .join()
        .expect("feeder thread must not panic")
        .expect("feed stdin");
    assert_eq!(out, prompt, "temp-file transport must round-trip exactly");
}

/// `E2BIG` is `7` on every Linux/Unix target Simard runs on; keep it named for
/// readability rather than pulling in the `libc` crate for one constant.
fn libc_e2big() -> i32 {
    7
}
