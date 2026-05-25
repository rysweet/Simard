//! Helper binary for `tests/cognitive_memory_cross_session_recall.rs` (issue #1974).
//!
//! Two-phase helper for cross-session recall testing. Takes a `--state-root`
//! directory and a `--phase` argument (`write` or `read`):
//!
//! ## Write phase
//! Opens `NativeCognitiveMemory::open(state_root)`, writes deterministic
//! entries across all four memory tiers (facts, episodes, working memory,
//! sensory), prints `DONE` to stdout, and exits cleanly.
//!
//! ## Read phase
//! Opens a *fresh* `NativeCognitiveMemory::open(state_root)` on the same
//! directory, queries each tier, and prints each recovered record as a JSON
//! line to stdout (one line per tier), followed by `DONE`.
//!
//! The parent integration test spawns write then read as separate child
//! processes against the same state root and asserts field-identical recall.
//!
//! Used only by the integration test; not built by default consumers.

use std::path::PathBuf;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut state_root: Option<PathBuf> = None;
    let mut phase: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state-root" => {
                state_root = Some(PathBuf::from(
                    args.next().expect("--state-root requires a value"),
                ));
            }
            "--phase" => {
                phase = Some(args.next().expect("--phase requires a value"));
            }
            other => panic!("unknown argument: {other}"),
        }
    }

    let state_root = state_root.expect("usage: helper --state-root <dir> --phase {write,read}");
    let phase = phase.expect("usage: helper --state-root <dir> --phase {write,read}");

    std::fs::create_dir_all(&state_root).expect("create state_root");

    let mem = NativeCognitiveMemory::open(&state_root).expect("open native cognitive memory");

    use std::io::Write;

    match phase.as_str() {
        "write" => {
            // --- Facts (semantic memory) ---
            for i in 0..3 {
                mem.store_fact(
                    &format!("recall-fact-{i}"),
                    &format!("fact-content-{i}"),
                    0.80 + (i as f64) * 0.05,
                    &[format!("tag-{i}"), "cross-session".to_string()],
                    "cross-session-test",
                )
                .expect("store_fact");
            }

            // --- Episodes (episodic memory) ---
            for i in 0..3 {
                mem.store_episode(&format!("recall-episode-{i}"), "cross-session-test", None)
                    .expect("store_episode");
            }

            // --- Working memory ---
            for i in 0..3 {
                mem.push_working(
                    &format!("slot-type-{i}"),
                    &format!("working-content-{i}"),
                    "cross-session-task",
                    0.5 + (i as f64) * 0.1,
                )
                .expect("push_working");
            }

            // --- Sensory memory (long TTL so it survives the read phase) ---
            for i in 0..3 {
                mem.record_sensory(
                    &format!("modality-{i}"),
                    &format!("sensory-data-{i}"),
                    3600, // 1 hour TTL — plenty of time for the read phase
                )
                .expect("record_sensory");
            }

            // Explicit checkpoint before clean exit to flush WAL.
            mem.checkpoint().expect("checkpoint");

            println!("DONE");
            let _ = std::io::stdout().flush();
        }
        "read" => {
            // --- Facts ---
            let facts = mem
                .search_facts("recall-fact", 100, 0.0)
                .expect("search_facts");
            let facts_json = serde_json::to_string(&facts).expect("serialize facts");
            println!("FACTS {facts_json}");

            // --- Working memory ---
            let working = mem.get_working("cross-session-task").expect("get_working");
            let working_json = serde_json::to_string(&working).expect("serialize working");
            println!("WORKING {working_json}");

            // --- Statistics (covers episodes + sensory counts) ---
            let stats = mem.get_statistics().expect("get_statistics");
            let stats_json = serde_json::to_string(&stats).expect("serialize stats");
            println!("STATS {stats_json}");

            println!("DONE");
            let _ = std::io::stdout().flush();
        }
        other => panic!("unknown phase: {other} — expected 'write' or 'read'"),
    }
}
