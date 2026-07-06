//! Integration regression test for the **no-PTY** engineer subprocess contract
//! (issue #2640).
//!
//! ## What this file pins
//!
//! The E2BIG ("Argument list too long") fix moved the engineer prompt off of
//! `argv` and onto the child's **STDIN pipe**. A pipe is not a pseudo-terminal:
//! the spawned `amplihack copilot --subprocess-safe …` subprocess runs
//! *headless*, with stdin, stdout, and stderr all wired to plain pipes and NO
//! controlling PTY. This test proves that transport does not secretly depend on
//! a pseudo-terminal:
//!
//!   1. The child's fd 0/1/2 are all NON-terminals (`[ -t N ]` is false) —
//!      i.e. there is no PTY anywhere in the spawn path.
//!   2. Despite the absence of a PTY, the (possibly large) prompt is still
//!      delivered losslessly on the stdin pipe and read to EOF by the child.
//!   3. `run_engineer_subprocess` returns `Ok(_)` — the headless, no-PTY child
//!      completes successfully.
//!
//! If a future refactor re-introduced a PTY (e.g. to make copilot "think" it is
//! interactive), the stdin EOF that tells the child to start work could be lost
//! and the hourly journal / OODA E2BIG-class hangs would return by a different
//! door. This test fails loudly if any std fd becomes a terminal.
//!
//! ## How the assertion is observed
//!
//! Like `tests/engineer_copilot_permissions.rs`, this does NOT invoke the real
//! Copilot CLI. It redirects the binary lookup (`SIMARD_AMPLIHACK_BIN`) at a
//! bash *stub* that records, for each standard fd, whether it is a TTY, drains
//! the prompt from stdin, and exits 0.

use std::path::{Path, PathBuf};

use serial_test::serial;
use simard::engineer_loop::{AgentKind, run_engineer_subprocess};

/// RAII guard for `SIMARD_AMPLIHACK_BIN`. Restores the prior value (or removes
/// the variable) on `Drop` so concurrent / subsequent tests are unaffected by
/// an uncaught panic mid-test.
struct AmplihackBinEnv {
    prior: Option<String>,
}

impl AmplihackBinEnv {
    fn set(value: &Path) -> Self {
        let prior = std::env::var("SIMARD_AMPLIHACK_BIN").ok();
        // SAFETY: tests that touch this var are serialized on the
        // `simard_amplihack_bin_env` key, so no two mutate it concurrently.
        unsafe {
            std::env::set_var("SIMARD_AMPLIHACK_BIN", value);
        }
        Self { prior }
    }
}

impl Drop for AmplihackBinEnv {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        unsafe {
            match self.prior.take() {
                Some(v) => std::env::set_var("SIMARD_AMPLIHACK_BIN", v),
                None => std::env::remove_var("SIMARD_AMPLIHACK_BIN"),
            }
        }
    }
}

/// Write a bash shim at `dir/amplihack` that records the TTY status of each
/// standard fd plus the stdin payload to `dir/observations.log`, then exits 0.
///
/// `[ -t N ]` is true iff fd N is connected to a terminal. Under the E2BIG-safe
/// spawn path every std fd is a pipe, so all three checks must report `notty`.
fn write_no_pty_probe_shim(dir: &Path) -> PathBuf {
    let shim_path = dir.join("amplihack");
    let log_path = dir.join("observations.log");
    let log_path_str = log_path.to_string_lossy();
    let script = format!(
        r#"#!/usr/bin/env bash
set -uo pipefail
LOG="{log}"
{{
    # Terminal status of each standard fd. A pseudo-terminal would make one or
    # more of these report `tty`; the no-PTY transport requires all `notty`.
    if [ -t 0 ]; then echo "FD0: tty"; else echo "FD0: notty"; fi
    if [ -t 1 ]; then echo "FD1: tty"; else echo "FD1: notty"; fi
    if [ -t 2 ]; then echo "FD2: tty"; else echo "FD2: notty"; fi
    # Drain the prompt delivered on the stdin pipe (issue #2640). `cat` blocks
    # until the feeder thread closes stdin (EOF) — proving the pipe carries the
    # prompt and terminates cleanly with no PTY in the loop.
    echo "STDIN_BEGIN"
    cat
    echo ""
    echo "STDIN_END"
    echo "DONE"
}} >> "$LOG"
exit 0
"#,
        log = log_path_str
    );
    std::fs::write(&shim_path, script).expect("write amplihack no-pty shim");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).unwrap();
    }

    shim_path
}

fn read_observations(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("observations.log")).expect("observations.log written")
}

/// Extract the `STDIN_BEGIN..STDIN_END` payload section from the shim log.
fn stdin_section(log: &str) -> String {
    log.split_once("STDIN_BEGIN")
        .and_then(|(_, rest)| rest.split_once("STDIN_END"))
        .map(|(stdin, _)| stdin.to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// No std fd is a PTY, yet the prompt still arrives on stdin and the run is Ok.
// ---------------------------------------------------------------------------

#[test]
#[serial(simard_amplihack_bin_env)]
fn engineer_subprocess_runs_headless_with_no_pty_on_any_std_fd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = write_no_pty_probe_shim(dir.path());
    let _guard = AmplihackBinEnv::set(&shim);

    let prompt = "no-pty engineer prompt: read me from the stdin pipe, not a terminal";
    let result = run_engineer_subprocess(prompt, dir.path(), AgentKind::Copilot);
    assert!(
        result.is_ok(),
        "the headless, no-PTY copilot subprocess must complete Ok; got: {result:?}"
    );

    let log = read_observations(dir.path());

    // The core no-PTY contract: none of the child's std fds may be a terminal.
    assert!(
        log.contains("FD0: notty"),
        "child STDIN must be a pipe, not a PTY (issue #2640); observations:\n{log}"
    );
    assert!(
        log.contains("FD1: notty"),
        "child STDOUT must be a pipe, not a PTY (issue #2640); observations:\n{log}"
    );
    assert!(
        log.contains("FD2: notty"),
        "child STDERR must be a pipe, not a PTY (issue #2640); observations:\n{log}"
    );
    assert!(
        !log.contains(": tty"),
        "no standard fd may be attached to a pseudo-terminal; observations:\n{log}"
    );

    // The prompt still arrives losslessly on the (non-PTY) stdin pipe.
    assert!(
        stdin_section(&log).contains(prompt),
        "the prompt must be delivered on the no-PTY stdin pipe (issue #2640); \
         observations:\n{log}"
    );
}

// ---------------------------------------------------------------------------
// The no-PTY stdin transport is size-independent: a large prompt that would
// have overflowed ARG_MAX on argv still round-trips byte-for-byte through the
// pipe with no PTY. This ties the no-PTY contract to the E2BIG fix it defends.
// ---------------------------------------------------------------------------

#[test]
#[serial(simard_amplihack_bin_env)]
fn large_prompt_round_trips_through_the_no_pty_stdin_pipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = write_no_pty_probe_shim(dir.path());
    let _guard = AmplihackBinEnv::set(&shim);

    // Well past a typical single-argument ARG_MAX ceiling (128 KiB > 128 KiB
    // MAX_ARG_STRLEN on Linux), so if this were ever inlined into argv the
    // spawn would fail with E2BIG. A unique marker at the tail proves the pipe
    // delivered the whole payload, not a truncated prefix.
    let mut prompt = "X".repeat(128 * 1024);
    prompt.push_str("::NO_PTY_TAIL_MARKER::");

    let result = run_engineer_subprocess(&prompt, dir.path(), AgentKind::Copilot);
    assert!(
        result.is_ok(),
        "a large prompt on the no-PTY stdin pipe must still complete Ok \
         (never E2BIG); got: {result:?}"
    );

    let log = read_observations(dir.path());
    assert!(
        log.contains("FD0: notty") && !log.contains(": tty"),
        "the large-prompt path must also use a no-PTY stdin pipe; observations \
         (truncated):\n{}",
        &log[..log.len().min(400)]
    );
    let stdin = stdin_section(&log);
    assert!(
        stdin.contains("::NO_PTY_TAIL_MARKER::"),
        "the tail of a large prompt must survive the stdin pipe intact — proof \
         the no-PTY transport is size-independent (issue #2640)"
    );
    assert!(
        stdin.matches('X').count() >= 128 * 1024,
        "the full large-prompt body must round-trip through the no-PTY pipe"
    );
}
