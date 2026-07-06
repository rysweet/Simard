//! Per-path E2BIG contract for the **engineer** subprocess launch (issue #2640).
//!
//! `engineer_loop::agent_spawn::run_engineer_subprocess` used to build the
//! `amplihack copilot` argv by inlining the whole engineer prompt as a trailing
//! `-p <prompt>` argv token (spawned with `stdin(Stdio::null())`). The engineer
//! prompt is `objective + repo inspection + guidelines`, which for a large goal
//! grows past the kernel's 128 KiB per-argument limit — so `exec` failed with
//! E2BIG ("Argument list too long") before the agent ever started. Same root
//! cause, different launch site.
//!
//! The fix makes `engineer_argv(Copilot, …)` **prompt-less** and routes the
//! prompt through the single spawn facade (`simard::spawn_payload`) on
//! **stdin**, so it never contributes to argv. This test pins both halves:
//!   1. THE FIX — the Copilot `engineer_argv` is prompt-less and stays far under
//!      ARG_MAX even for a 0.5 MiB prompt (execs with no E2BIG); and
//!   2. THE FIX — the SAME oversized prompt, delivered via the facade on stdin,
//!      round-trips through a real child with no size failure.
//!
//! TDD status: RED until `simard::spawn_payload` exists. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

use simard::engineer_loop::{AgentKind, engineer_argv};
use simard::prompt_delivery::PromptDelivery;
use simard::spawn_payload;

const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB > 128 KiB per-arg limit

fn oversized_engineer_prompt(marker: &str) -> String {
    format!("{marker} objective + repo inspection — ")
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect()
}

/// THE FIX: `engineer_argv(Copilot, …)` is now prompt-less — even an oversized
/// prompt produces a tiny, bounded argv (the prompt rides on stdin, see
/// `oversized_engineer_prompt_via_facade_stdin_spawns_successfully`), so `exec`
/// can never fail with E2BIG. Spawning the real argv against `/bin/echo` proves
/// the invocation stays far under ARG_MAX regardless of prompt size.
#[test]
fn fixed_engineer_argv_is_prompt_less_and_arg_max_safe() {
    let marker = "ENGINEER-FIX-MARKER-2640";
    let prompt = oversized_engineer_prompt(marker);
    let argv = engineer_argv(AgentKind::Copilot, &prompt, 5);

    // The prompt is NOT inlined into argv — this is the defect the fix removed
    // (the prompt is delivered on stdin instead).
    assert!(
        !argv.iter().any(|a| a.contains(marker)),
        "issue #2640: the Copilot engineer argv must be prompt-less: {argv:?}"
    );
    // The whole argv stays tiny even with a 0.5 MiB prompt in flight.
    let total: usize = argv.iter().map(|a| a.len() + 1).sum();
    assert!(
        total < 16 * 1024,
        "engineer argv must stay far under ARG_MAX with a 0.5 MiB prompt: {total} bytes"
    );

    // And it actually execs successfully (no E2BIG) because nothing large is on
    // argv — the exact failure mode the fix eliminates.
    let status = Command::new("/bin/echo")
        .args(&argv)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("the prompt-less engineer argv must exec without E2BIG");
    assert!(status.success());
}

/// THE FIX: the SAME oversized engineer prompt, delivered via the facade on
/// stdin, round-trips byte-for-byte through a real child — no E2BIG, and no
/// prompt bytes on argv.
#[test]
fn oversized_engineer_prompt_via_facade_stdin_spawns_successfully() {
    let marker = "ENGINEER-FIX-MARKER-2640";
    let prompt = oversized_engineer_prompt(marker).into_bytes();

    // Model the agent sink with /bin/cat; the transport (stdin) is what the fixed
    // engineer launch uses.
    let mut cmd = Command::new("/bin/cat");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let applied = spawn_payload::attach_prompt_std(&mut cmd, &prompt)
        .expect("facade stdin delivery must not fail for an oversized engineer prompt");
    assert_ne!(
        applied.mode(),
        PromptDelivery::Inline,
        "an oversized engineer prompt must not be delivered Inline"
    );
    assert!(
        !cmd.get_args().any(|a| a.to_string_lossy().contains(marker)),
        "the engineer prompt must never appear in argv"
    );

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
        "the engineer prompt must be delivered in full via stdin, no truncation"
    );
}
