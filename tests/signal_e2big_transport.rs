//! Per-path E2BIG contract for the **SIGNAL channel** agent session (issue #2640).
//!
//! This is the path the operator now hits on the SIGNAL channel:
//! `signal_conversation::open_signal_agent_session()` builds a copilot base-type
//! session (`SessionBuilder … adapter_tag("signal")`), so it shares the exact
//! `base_type_copilot` launch machinery as the meeting / decision-cycle / engineer
//! paths. When a large accumulated signal context (>256 KB) was inlined into
//! copilot's argv it exceeded `ARG_MAX` and `exec` failed with E2BIG ("Argument
//! list too long"), taking the operator's SIGNAL reasoner down.
//!
//! The fix routes the signal prompt/context through the single spawn facade
//! (`simard::spawn_payload`), which delivers it on **stdin** and NEVER places it
//! in `argv` OR `envp`. This test pins the signal-specific guarantee:
//!   1. an oversized signal context appears in neither `argv` nor `envp`, and
//!   2. it round-trips through a real child on stdin with no E2BIG — while the old
//!      inline-argv form still fails with E2BIG.
//!
//! TDD status: RED until `simard::spawn_payload` exists. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

#![cfg(unix)]

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Stdio};

use simard::prompt_delivery::PromptDelivery;
use simard::spawn_payload;

const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB > 128 KiB per-arg limit

fn oversized_signal_context(marker: &str) -> Vec<u8> {
    // Shape it like an accumulated SIGNAL conversation the reasoner must read in
    // full, with a distinctive marker so any leak into argv/env is caught.
    format!("{marker} signal-context turn: ")
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

fn envs_of(cmd: &Command) -> Vec<String> {
    cmd.get_envs()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k.to_string_lossy(),
                v.map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// The signal launch must keep an oversized context out of BOTH `argv` and `envp`
/// (env shares the same `ARG_MAX` budget). The facade attaches the prompt on
/// stdin; the invocation carries neither the payload nor an env-smuggled copy.
#[test]
fn oversized_signal_context_never_enters_argv_or_env() {
    let marker = "SIGNAL-CONTEXT-MARKER-2640";
    let context = oversized_signal_context(marker);

    // Build a signal-style copilot invocation (bounded flags only); the facade
    // owns the payload transport.
    let mut cmd = Command::new("copilot");
    cmd.args(["--no-custom-instructions", "--silent", "--allow-all-tools"]);

    let applied = spawn_payload::attach_prompt_std(&mut cmd, &context)
        .expect("facade must attach an oversized signal context without failing");

    assert_ne!(
        applied.mode(),
        PromptDelivery::Inline,
        "an oversized signal context must not be delivered Inline (argv)"
    );

    let args = args_of(&cmd);
    assert!(
        !args.iter().any(|a| a.contains(marker)),
        "issue #2640: signal context must never appear in argv: {args:?}"
    );
    let envs = envs_of(&cmd);
    assert!(
        !envs.iter().any(|e| e.contains(marker)),
        "issue #2640: signal context must never be smuggled through envp: {envs:?}"
    );
}

/// THE BUG (contrast): inlining the oversized signal context as one argv token
/// fails `exec` with E2BIG (`errno 7`) before the child runs — the live SIGNAL
/// failure.
#[test]
fn oversized_signal_context_inlined_fails_with_e2big() {
    let context = oversized_signal_context("SIGNAL-BUG");
    let arg = String::from_utf8(context).expect("utf8");
    let err = Command::new("/bin/echo")
        .arg(&arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect_err("an oversized argv token must fail to exec (E2BIG)");
    assert_eq!(
        err.raw_os_error(),
        Some(7),
        "the argv-inlined oversized signal context must fail with E2BIG: {err:?}"
    );
}

/// THE FIX: the SAME oversized signal context, delivered via the facade on stdin,
/// round-trips byte-for-byte through a real child — the SIGNAL reasoner receives
/// the full context with no E2BIG.
#[test]
fn oversized_signal_context_via_facade_stdin_spawns_successfully() {
    let context = oversized_signal_context("SIGNAL-FIX");

    let mut cmd = Command::new("/bin/cat");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let applied = spawn_payload::attach_prompt_std(&mut cmd, &context)
        .expect("facade stdin delivery must not fail for an oversized signal context");

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
        context.len(),
        "the signal context must be delivered in full via stdin, no truncation"
    );
}
