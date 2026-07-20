//! TDD regression tests for issue #4360 (P3): `run_command_inner` pipe-drain
//! deadlock.
//!
//! The pre-fix `run_command_inner` polls `child.try_wait()` in a loop and only
//! reads the child's stdout/stderr *after* the child has exited (via
//! `wait_with_output`). When a child writes more than the OS pipe buffer
//! (~64 KiB on Linux) to stdout **or** stderr, that pipe fills, the child
//! blocks on `write`, and it can never exit — so the poll loop spins until the
//! command timeout (60s) fires and returns `Err(CommandTimeout)`.
//!
//! These tests spawn children that emit well over 64 KiB and assert the call
//! completes promptly with the full output captured. They FAIL against the
//! buggy implementation (they hang until the timeout and return an error) and
//! PASS once stdout/stderr are drained concurrently while polling.
//!
//! Contract asserted (additive / non-breaking):
//! - `run_command` returns `Ok(CommandOutput)` for a zero-exit child regardless
//!   of output volume on either stream.
//! - Full stdout is captured (no truncation of the drained stream).
//! - stderr volume alone does not stall the call.
//! - Small-output behavior and exact stdout are preserved (ordering/return).

use std::path::Path;
use std::time::{Duration, Instant};

use super::execution::run_command;

/// Directory to run the child in. Any existing dir works; use the crate root.
fn cwd() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Generous upper bound: the fix should finish in well under a second, but we
/// only need to prove we are not hitting the 60s command timeout.
const NO_DEADLOCK_BUDGET: Duration = Duration::from_secs(30);

/// ~200 KiB of stdout (100_000 lines of "a\n"), far above the ~64 KiB pipe
/// buffer. Pre-fix: stdout pipe fills, child blocks, poll loop times out.
#[test]
fn large_stdout_does_not_deadlock_issue_4360() {
    let start = Instant::now();
    let result = run_command(cwd(), &["sh", "-c", "yes a | head -n 100000"]);
    let elapsed = start.elapsed();

    let output = result.expect("large stdout child must not deadlock or time out");
    let a_count = output.stdout.matches('a').count();
    assert!(
        a_count >= 100_000,
        "expected full stdout capture (>=100000 'a'), got {a_count}"
    );
    assert!(
        elapsed < NO_DEADLOCK_BUDGET,
        "call took {elapsed:?}, indicating a pipe-drain deadlock"
    );
}

/// ~200 KiB written to stderr with a tiny stdout marker, child exits 0.
/// Pre-fix: stderr pipe fills, child blocks, poll loop times out.
#[test]
fn large_stderr_does_not_deadlock_issue_4360() {
    let start = Instant::now();
    let result = run_command(
        cwd(),
        &["sh", "-c", "yes b | head -n 100000 1>&2; printf done"],
    );
    let elapsed = start.elapsed();

    let output = result.expect("large stderr child must not deadlock or time out");
    assert_eq!(
        output.stdout.trim(),
        "done",
        "stdout marker must be captured after a large stderr burst"
    );
    assert!(
        elapsed < NO_DEADLOCK_BUDGET,
        "call took {elapsed:?}, indicating a pipe-drain deadlock on stderr"
    );
}

/// Both streams emit well over 64 KiB concurrently; child exits 0.
#[test]
fn large_both_streams_do_not_deadlock_issue_4360() {
    let start = Instant::now();
    let result = run_command(
        cwd(),
        &[
            "sh",
            "-c",
            "yes b | head -n 100000 1>&2 & yes a | head -n 100000; wait",
        ],
    );
    let elapsed = start.elapsed();

    let output = result.expect("large dual-stream child must not deadlock or time out");
    let a_count = output.stdout.matches('a').count();
    assert!(
        a_count >= 100_000,
        "expected full stdout capture (>=100000 'a') with concurrent stderr load, got {a_count}"
    );
    assert!(
        elapsed < NO_DEADLOCK_BUDGET,
        "call took {elapsed:?}, indicating a pipe-drain deadlock under dual-stream load"
    );
}

/// Non-breaking guard: small commands still return exact stdout unchanged.
#[test]
fn small_output_behavior_preserved_issue_4360() {
    let output = run_command(cwd(), &["sh", "-c", "printf hello"])
        .expect("small command must still succeed");
    assert_eq!(output.stdout, "hello");
}

/// Memory-safety backstop: a child emitting far more than the 16 MiB per-stream
/// capture cap returns promptly (no deadlock), with the retained buffer clipped
/// to the cap and a truncation marker appended — the drain still continues past
/// the cap so the child never blocks.
#[test]
fn capture_cap_truncates_and_marks_issue_4360() {
    const CAP_BYTES: usize = 16 * 1024 * 1024;
    // Emit ~20 MiB on stdout, well past the 16 MiB cap.
    let start = Instant::now();
    let result = run_command(cwd(), &["sh", "-c", "yes a | head -c 20971520"]);
    let elapsed = start.elapsed();

    let output = result.expect("oversized stdout must not deadlock or error");
    assert!(
        output.stdout.contains("output truncated at 16 MiB"),
        "clipped capture must carry the truncation marker"
    );
    assert!(
        output.stdout.len() <= CAP_BYTES + 128,
        "retained buffer must be bounded near the cap, got {} bytes",
        output.stdout.len()
    );
    assert!(
        elapsed < NO_DEADLOCK_BUDGET,
        "call took {elapsed:?}, indicating a deadlock past the capture cap"
    );
}
