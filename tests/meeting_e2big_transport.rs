//! Per-path E2BIG contract for the **meeting** turn (issue #2640).
//!
//! `base_type_copilot::run_meeting_turn` used to launch copilot via
//! `sh -c "copilot … -p \"$(cat 'PATH')\""` — the `$(cat …)` command-substitution
//! expanded the whole prompt into argv and, for a large meeting context (>256 KB),
//! blew past `ARG_MAX` so `exec` failed with E2BIG ("Argument list too long").
//!
//! The fix routes the meeting prompt through the single spawn facade
//! (`simard::spawn_payload`), which delivers it on **stdin**; the invocation
//! `argv` (built by [`build_meeting_command`]) carries only bounded flags. This
//! test pins both halves:
//!   1. the meeting invocation `argv` never carries the prompt (E2BIG-safe by
//!      construction), and
//!   2. the SAME oversized prompt, delivered via the facade, round-trips through a
//!      real child on stdin with no size failure — while the old inline-argv form
//!      still fails with E2BIG.
//!
//! TDD status: RED until `simard::spawn_payload` exists. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

#![cfg(unix)]

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Stdio};

use simard::base_type_copilot::build_meeting_command;
use simard::prompt_delivery::PromptDelivery;
use simard::spawn_payload;

const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB > 128 KiB per-arg limit
const ARGV_SAFE_CEILING: usize = 16 * 1024; // 16 KiB, far under ARG_MAX

fn oversized_meeting_prompt(marker: &str) -> Vec<u8> {
    format!("{marker} meeting objective — ")
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect::<String>()
        .into_bytes()
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a: &OsStr| a.to_string_lossy().into_owned())
        .collect()
}

/// The meeting command must be a direct, bounded-argv copilot invocation: no
/// `sh -c`, no `-p`, no `$(cat …)`, and a total argv far under ARG_MAX. Even
/// after attaching a 0.5 MiB prompt through the facade, the argv stays tiny
/// because the prompt goes on stdin.
#[test]
fn meeting_argv_is_bounded_after_attaching_an_oversized_prompt() {
    let marker = "MEETING-PROMPT-MARKER-2640";
    let prompt = oversized_meeting_prompt(marker);

    let mut cmd = build_meeting_command("copilot", "sess-2640", Some("/tmp/work"));
    let applied = spawn_payload::attach_prompt_std(&mut cmd, &prompt)
        .expect("facade must attach an oversized meeting prompt without failing");

    // Never inlined onto argv.
    assert_ne!(
        applied.mode(),
        PromptDelivery::Inline,
        "an oversized meeting prompt must not be delivered Inline"
    );

    let args = args_of(&cmd);
    assert!(
        !args.iter().any(|a| a == "-p"),
        "issue #2640: meeting argv must not pass the prompt via `-p`: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("$(cat")),
        "issue #2640: meeting argv must not inline `$(cat …)`: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains(marker)),
        "the meeting prompt body must never appear in argv"
    );

    let total: usize =
        cmd.get_program().to_string_lossy().len() + args.iter().map(|a| a.len() + 1).sum::<usize>();
    assert!(
        total < ARGV_SAFE_CEILING,
        "meeting argv must stay far under ARG_MAX even with a 0.5 MiB prompt in \
         flight: {total} bytes >= {ARGV_SAFE_CEILING}"
    );
}

/// THE BUG (contrast): the old inline form — the whole meeting prompt as one
/// argv token — fails `exec` with E2BIG (`errno 7`) before the child runs.
#[test]
fn oversized_meeting_prompt_inlined_fails_with_e2big() {
    let prompt = oversized_meeting_prompt("MEETING-BUG");
    let arg = String::from_utf8(prompt).expect("utf8");
    let err = Command::new("/bin/echo")
        .arg(&arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect_err("an oversized argv token must fail to exec (E2BIG)");
    assert_eq!(
        err.raw_os_error(),
        Some(7),
        "the argv-inlined oversized meeting prompt must fail with E2BIG: {err:?}"
    );
}

/// THE FIX: the SAME oversized meeting prompt, delivered via the facade on stdin,
/// round-trips byte-for-byte through a real child — no E2BIG.
#[test]
fn oversized_meeting_prompt_via_facade_stdin_spawns_successfully() {
    let prompt = oversized_meeting_prompt("MEETING-FIX");

    // Model the copilot sink with /bin/cat so the test is hermetic; the transport
    // (stdin) is identical to what the real meeting turn uses.
    let mut cmd = Command::new("/bin/cat");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let applied = spawn_payload::attach_prompt_std(&mut cmd, &prompt)
        .expect("facade stdin delivery must not fail for an oversized meeting prompt");

    let mut child = cmd.spawn().expect("spawn /bin/cat");
    let stdin = child.stdin.take();
    let feeder = std::thread::spawn(move || applied.feed(stdin));

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_end(&mut out)
        .expect("read stdout");
    assert!(child.wait().expect("wait").success());
    feeder.join().expect("feeder").expect("feed stdin");

    assert_eq!(
        out.len(),
        prompt.len(),
        "the meeting prompt must be delivered in full via stdin, no truncation"
    );
}
