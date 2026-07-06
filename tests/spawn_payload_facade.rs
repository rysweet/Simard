//! Failing TDD contract for the single large-payload spawn **facade**
//! (`simard::spawn_payload`), issue #2640.
//!
//! The E2BIG ("Argument list too long", `errno 7`) failure has been fixed
//! piecemeal at individual launch sites (#2660 copilot/OODA stdin, #2692/#2700
//! journal recipe file-channel) but keeps recurring because there is no *single*
//! chokepoint every agent/recipe launch is forced through. This file pins the
//! net-new facade that closes the class: one module, one policy —
//!
//! > a dynamic value whose length can reach [`ARGV_PAYLOAD_MAX_BYTES`] is
//! > delivered out-of-band (copilot prompts on **stdin**, recipe context on a
//! > **file** referenced by `-c <key>_path=<abs>`) and never appears in `argv`
//! > or `envp`.
//!
//! The facade adds NO new byte-transport: it composes the two already-shipping
//! modules — [`simard::prompt_delivery`] (`apply_std`/`apply_tokio`) and
//! [`simard::recipe_context_file`] (`ContextFile::write`) — under one policy, and
//! surfaces every pre-exec spawn error through the existing errno classifier +
//! [`simard::overseer::failure_sink`] (no silent fallback).
//!
//! Full wire-level contract: `docs/reference/large-payload-spawn-api.md`.
//!
//! TDD status: RED until the fix adds `src/spawn_payload/mod.rs` and registers
//! `pub mod spawn_payload;` in `src/lib.rs`. Isolated integration crate — the red
//! compile does not affect the rest of the suite (same convention as
//! `tests/ooda_e2big_transport.rs` / `tests/journal_e2big_transport.rs`).

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

use simard::overseer::diagnosis::FailureCause;
use simard::overseer::failure_sink;
use simard::prompt_delivery::{INLINE_MAX_BYTES, PromptDelivery};
use simard::spawn_payload::{self, RecipeArg};

/// A payload larger than Linux `MAX_ARG_STRLEN` (128 KiB per argument) AND larger
/// than the > 256 KiB threshold in the issue's verification note. If any facade
/// path let this touch argv/envp, `exec` would fail with E2BIG.
const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB

/// A conservative ceiling well under any real `ARG_MAX` (Linux total ~2 MiB,
/// per-arg 128 KiB). A facade-built argument must stay far below this even when a
/// 0.5 MiB payload backs it — proving the payload is NOT on argv.
const ARGV_SAFE_CEILING: usize = 16 * 1024; // 16 KiB

fn oversized(marker: &str) -> String {
    format!("{marker}:")
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect()
}

// ---------------------------------------------------------------------------
// Policy constant
// ---------------------------------------------------------------------------

/// The single policy threshold is 8 KiB and coincides with
/// `prompt_delivery::INLINE_MAX_BYTES`, so "small = may inline" is one boundary
/// for both prompts and recipe context.
#[test]
fn argv_payload_max_is_eight_kib_and_matches_prompt_inline_cap() {
    assert_eq!(
        spawn_payload::ARGV_PAYLOAD_MAX_BYTES,
        8 * 1024,
        "the shared policy threshold must be 8 KiB"
    );
    assert_eq!(
        spawn_payload::ARGV_PAYLOAD_MAX_BYTES,
        INLINE_MAX_BYTES,
        "the facade threshold must equal prompt_delivery::INLINE_MAX_BYTES so the \
         inline boundary is identical for prompts and recipe context"
    );
}

// ---------------------------------------------------------------------------
// Context transport (recipe-runner family) — recipe_context / RecipeArg
// ---------------------------------------------------------------------------

/// A small context value stays inline (`key=value`), collapsed to one line for
/// YAML safety (#2127) but NEVER truncated (it is already small).
#[test]
fn recipe_context_inlines_small_values() {
    let arg = spawn_payload::recipe_context("facade", "goal_id", "abc-123")
        .expect("small inline resolution must not fail");

    match &arg {
        RecipeArg::Inline(v) => {
            assert_eq!(v, "goal_id=abc-123", "small value must inline verbatim");
        }
        RecipeArg::Filed(_) => panic!("a 7-byte value must NOT be filed: {arg:?}"),
    }
    assert_eq!(arg.arg_value(), "goal_id=abc-123");
}

/// A small multi-line value inlines with newlines collapsed so a multi-line brief
/// can never break the recipe YAML interpolation (#2127).
#[test]
fn recipe_context_collapses_newlines_in_small_inline_values() {
    let arg = spawn_payload::recipe_context("facade", "note", "line one\nline two\n\tline three")
        .expect("resolve small multi-line value");
    let value = arg.arg_value();
    assert!(
        !value.contains('\n'),
        "inline recipe context must collapse newlines for YAML safety: {value:?}"
    );
    assert!(
        value.starts_with("note="),
        "inline value keeps its key: {value:?}"
    );
}

/// THE FIX (context): an oversized context value is written to a file and only a
/// short `<key>_path=<abs>` rides on argv — never the payload — and the payload
/// is recoverable byte-for-byte from the file (lossless; guideline G3 — no
/// truncation). This is the exact transport that ended the hourly journal E2BIG.
#[test]
fn recipe_context_files_oversized_values_losslessly() {
    let marker = "OVERSIZED-CONTEXT-MARKER-2640";
    let payload = oversized(marker);
    assert!(
        payload.len() > 256 * 1024,
        "payload must exceed the >256KB verification threshold"
    );

    let arg = spawn_payload::recipe_context("facade", "day_context", &payload)
        .expect("oversized context must file successfully, never fail");

    // Must be filed, not inlined.
    let cf = match &arg {
        RecipeArg::Filed(cf) => cf,
        RecipeArg::Inline(_) => panic!("an oversized value MUST be filed, not inlined: {arg:?}"),
    };

    // argv carries only the short path token.
    let value = arg.arg_value();
    assert!(
        value.starts_with("day_context_path="),
        "filed value must be `<key>_path=<abs>`: {value:?}"
    );
    assert!(
        value.len() < ARGV_SAFE_CEILING && !value.contains(marker),
        "the `-c` value must be a short path, not the inlined payload: {} bytes",
        value.len()
    );

    // The full payload is recoverable from the file — NOT truncated to 8 KiB.
    let on_disk = std::fs::read_to_string(cf.path()).expect("read context file");
    assert_eq!(
        on_disk.len(),
        payload.len(),
        "the file channel must persist the FULL payload with zero truncation"
    );
    assert_eq!(on_disk, payload, "round-tripped context must match exactly");
}

/// Even a 1 MiB context value yields a tiny argv token — ARG_MAX safety by
/// construction, independent of payload size.
#[test]
fn recipe_context_arg_value_is_arg_max_safe_for_huge_payloads() {
    let payload: String = "x".repeat(1024 * 1024);
    let arg = spawn_payload::recipe_context("facade", "plan", &payload).expect("file huge value");
    assert!(
        arg.arg_value().len() < ARGV_SAFE_CEILING,
        "a 1 MiB payload must still produce a tiny `_path` argv token"
    );
}

// ---------------------------------------------------------------------------
// Prompt transport (copilot family) — attach_prompt_std / attach_prompt_tokio
// ---------------------------------------------------------------------------

/// THE FIX (prompt): attaching an oversized prompt via the facade never inlines
/// it into argv (mode is Stdin/TempFile, not Inline), and the full prompt
/// round-trips through a real child on stdin with no E2BIG.
#[test]
fn attach_prompt_std_delivers_oversized_prompt_off_argv() {
    let marker = "OVERSIZED-PROMPT-MARKER-2640";
    let prompt = oversized(marker).into_bytes();

    let mut cmd = Command::new("/bin/cat");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let applied = spawn_payload::attach_prompt_std(&mut cmd, &prompt)
        .expect("facade prompt attach must not fail for an oversized prompt");

    assert_ne!(
        applied.mode(),
        PromptDelivery::Inline,
        "an oversized prompt must NEVER be delivered Inline (argv): mode was {:?}",
        applied.mode()
    );

    // argv must carry no fragment of the prompt.
    assert!(
        !cmd.get_args().any(|a| a.to_string_lossy().contains(marker)),
        "the facade must not inline any prompt bytes into argv"
    );

    // Real round-trip on stdin (feeder thread avoids the stdin/stdout deadlock).
    let mut child = cmd.spawn().expect("spawn /bin/cat");
    let stdin = child.stdin.take();
    let feeder = std::thread::spawn(move || applied.feed(stdin));

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_end(&mut out)
        .expect("read stdout");
    assert!(child.wait().expect("wait").success());
    feeder
        .join()
        .expect("feeder thread must not panic")
        .expect("feeding the oversized prompt on stdin must succeed");

    assert_eq!(
        out.len(),
        prompt.len(),
        "the stdin transport must deliver the full prompt byte-for-byte"
    );
}

/// A small prompt is allowed to inline (the facade uses `Auto`); the point of the
/// invariant is only that values that CAN exceed the cap never inline.
#[test]
fn attach_prompt_std_allows_small_prompts() {
    let mut cmd = Command::new("/bin/echo");
    let applied = spawn_payload::attach_prompt_std(&mut cmd, b"hello")
        .expect("small prompt attach must succeed");
    // Auto → Inline for a 5-byte prompt is fine; just assert it resolved a mode.
    let _ = applied.mode();
}

/// Async sibling: the tokio path likewise keeps an oversized prompt off argv and
/// round-trips it on stdin.
#[tokio::test]
async fn attach_prompt_tokio_delivers_oversized_prompt_off_argv() {
    use tokio::io::AsyncReadExt as _;

    let marker = "OVERSIZED-TOKIO-PROMPT-2640";
    let prompt = oversized(marker).into_bytes();

    let mut cmd = tokio::process::Command::new("/bin/cat");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let applied = spawn_payload::attach_prompt_tokio(&mut cmd, &prompt)
        .await
        .expect("async facade prompt attach must not fail");
    assert_ne!(
        applied.mode(),
        PromptDelivery::Inline,
        "async oversized prompt must not inline"
    );

    let mut child = cmd.spawn().expect("spawn /bin/cat (tokio)");
    let stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("stdout");
    let feeder = tokio::spawn(async move { applied.feed(stdin).await });

    let mut out = Vec::new();
    stdout.read_to_end(&mut out).await.expect("read stdout");
    let status = child.wait().await.expect("wait");
    feeder.await.expect("join feeder").expect("feed stdin");

    assert!(status.success());
    assert_eq!(out.len(), prompt.len(), "full prompt via tokio stdin");
}

// ---------------------------------------------------------------------------
// Failure surfacing — record_spawn_failure -> overseer::failure_sink
// ---------------------------------------------------------------------------

/// A pre-exec E2BIG spawn error (errno 7, no `ExitStatus`) must be classified and
/// recorded into the Overseer failure sink — never silently swallowed. This is
/// the "diagnose, don't just log" seam the facade guarantees for every launch.
#[test]
fn record_spawn_failure_surfaces_e2big_into_the_sink() {
    // Clear any prior entries so the assertion sees only our record.
    let _ = failure_sink::drain_recent();

    let e2big = std::io::Error::from_raw_os_error(7); // E2BIG
    spawn_payload::record_spawn_failure(&e2big, "facade-test-site");

    let recorded = failure_sink::drain_recent();
    assert!(
        recorded
            .iter()
            .any(|d| d.cause == FailureCause::ArgListTooLong && d.exit_code.is_none()),
        "an errno-7 spawn failure must be recorded as ArgListTooLong (exit_code None): {recorded:?}"
    );
}
