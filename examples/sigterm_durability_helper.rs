//! Helper binary for `tests/daemon_sigterm_durability.rs`.
//!
//! Opens a `NativeCognitiveMemory` writer at the path passed as `argv[1]`,
//! writes N=10 distinct facts (concept = `durability-fact-{i}`), prints a
//! single line `READY <pid>` to stdout (so the parent test knows it can send
//! the signal), then loops on a `ctrlc`-installed shutdown flag.
//!
//! On flag set (SIGTERM/SIGINT/SIGHUP), executes the daemon shutdown order:
//!   1. checkpoint (collapse WAL into main DB)
//!   2. clear in-process writer registration
//!   3. drop the strong Arc (Database::drop fires force_checkpoint_on_close)
//!
//! This mirrors what `shutdown_daemon` does in the real OODA daemon — without
//! requiring an LLM provider, dashboard, or python bridges.
//!
//! Used only by the integration test; not built by default consumers.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use simard::memory_ipc;

fn main() {
    let mut args = std::env::args().skip(1);
    let state_root = PathBuf::from(args.next().expect("usage: helper <state_root>"));
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    std::fs::create_dir_all(&state_root).expect("create state_root");

    let writer: Arc<dyn CognitiveMemoryOps> =
        Arc::new(NativeCognitiveMemory::open(&state_root).expect("open native cognitive memory"));
    memory_ipc::register_in_process_writer(state_root.clone(), Arc::clone(&writer));

    for i in 0..n {
        writer
            .store_fact(
                &format!("durability-fact-{i}"),
                &format!("payload-{i}"),
                0.95,
                &["sigterm-durability".to_string()],
                "sigterm-test",
            )
            .expect("store_fact");
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("install signal handler");
    }

    println!("READY {}", std::process::id());
    use std::io::Write;
    let _ = std::io::stdout().flush();

    while !shutdown.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
    }

    // Mirror shutdown_daemon order (checkpoint → clear writer → drop Arc).
    if let Err(e) = writer.checkpoint() {
        eprintln!("[helper] checkpoint failed: {e}");
    }
    memory_ipc::clear_in_process_writer();
    drop(writer);
    eprintln!("[helper] clean shutdown complete");
}
