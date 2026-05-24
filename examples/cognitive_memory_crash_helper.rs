//! Helper binary for `tests/cognitive_memory_crash_durability.rs` (issue #1973).
//!
//! Stripped-down counterpart of `examples/sigterm_durability_helper.rs`:
//! does NOT install a SIGTERM handler and does NOT call any shutdown sequence
//! (no `checkpoint()`, no `clear_in_process_writer`, no Arc drop). The whole
//! point is to model an unannounced crash mid-flight (SIGKILL / OOM / power
//! loss): the parent test asserts that the **per-write fsync barrier**
//! (issue #1973) has already made the write durable by the time `store_fact`
//! returns `Ok(())` — well before the print of `WROTE`.
//!
//! Protocol (line-delimited stdout, parent reads in order):
//!   1. `READY <pid>\n`  — printed before the write; tells parent the PID.
//!   2. `WROTE\n`        — printed AFTER `store_fact` returns `Ok(())`. The
//!      barrier (when implemented) has already issued
//!      `checkpoint() → fsync(data) → fsync(parent dir)`
//!      by this point, so the parent can SIGKILL safely.
//!   3. (blocks in `libc::pause()`) — never returns. The parent must signal.
//!
//! Argv: `helper <state_root> [<concept>] [<payload>]`
//!   - `concept` defaults to `"crash-marker"`
//!   - `payload` defaults to `"crash-payload"`
//!
//! Used only by the integration test; not built by default consumers.

use std::path::PathBuf;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

fn main() {
    let mut args = std::env::args().skip(1);
    let state_root = PathBuf::from(
        args.next()
            .expect("usage: helper <state_root> [concept] [payload]"),
    );
    let concept = args.next().unwrap_or_else(|| "crash-marker".to_string());
    let payload = args.next().unwrap_or_else(|| "crash-payload".to_string());

    std::fs::create_dir_all(&state_root).expect("create state_root");

    let writer = NativeCognitiveMemory::open(&state_root).expect("open native cognitive memory");

    use std::io::Write;

    // Announce PID *before* the write so the parent has a valid signal target
    // even if the write call hangs.
    println!("READY {}", std::process::id());
    let _ = std::io::stdout().flush();

    writer
        .store_fact(
            &concept,
            &payload,
            0.95,
            &["crash-durability".to_string()],
            "crash-test",
        )
        .expect("store_fact must return Ok(()) — barrier guarantees durability on Ok");

    // SAFETY-RELEVANT: do NOT call checkpoint(), do NOT clear the IPC writer,
    // do NOT drop the Arc explicitly. The whole test premise is that the
    // per-write barrier inside `store_fact` already made the data durable.
    // Any cleanup here would mask a missing barrier.

    println!("WROTE");
    let _ = std::io::stdout().flush();

    // Block until SIGKILL'd by the parent. `pause()` returns -1 with EINTR
    // only when a *handler* runs; SIGKILL has no handler, so this never
    // returns. We unwrap the loop just to satisfy the type checker on
    // platforms where pause() is typed as returning.
    #[cfg(unix)]
    loop {
        // SAFETY: libc::pause is a thin syscall wrapper, no FFI invariants
        // beyond "block until a signal is delivered".
        unsafe {
            libc::pause();
        }
    }
}
