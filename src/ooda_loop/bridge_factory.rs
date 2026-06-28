//! Construct an [`OodaBridges`] from a `state_root` path so that recipe
//! steps (which run as short-lived helper-bin invocations) can instantiate
//! the same bridges the long-lived OODA daemon uses.
//!
//! Strategy:
//!
//! 1. Memory: prefer the live [`RemoteCognitiveMemory`] IPC client at
//!    `socket_path_for(state_root)` so we share the daemon's open store
//!    handle when one is running. If that fails, fall back to a direct
//!    [`LibraryCognitiveMemory::open`] on `state_root`. The fallback is the
//!    correct behaviour for one-shot recipe runs (parity tests, ad-hoc
//!    `amplihack recipe run`) when no daemon is up.
//! 2. Knowledge / gym: launch native Rust transports via
//!    [`crate::bridge_launcher`]. These are in-process and incur negligible
//!    startup cost compared to the former Python subprocess approach.
//! 3. Session: not constructed here. LLM sessions are heavyweight and only
//!    the long-running daemon needs one. Recipe steps that need agent
//!    delegation should use `type: recipe` to dispatch to the
//!    `simard-engineer-loop` recipe instead.
//!
//! This module is the bridge between the daemon's bespoke wiring (in
//! `operator_commands_ooda::daemon`) and the recipe-runner's stateless
//! helper-bin model. Both paths share `bridge_launcher` for the native
//! Rust transports; they differ only in how memory and the LLM session
//! are obtained.

use std::path::Path;
use std::sync::Arc;

use crate::bridge_launcher::{launch_gym_bridge_native, launch_knowledge_bridge_native};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::SimardResult;
use crate::memory_ipc::{self, RemoteCognitiveMemory, SharedMemory};

use super::OodaBridges;

/// Connect to the live IPC memory server if one is running, otherwise open
/// the library-backed cognitive-memory store directly.
///
/// De-fork Phase 2b (issue #2307): [`LibraryCognitiveMemory`] (amplihack-
/// memory-lib, lbug-backed) is the sole cognitive-memory backend. The native
/// LadybugDB fork has been deleted; there is no env gate and no fallback.
///
/// Returned as `Box<dyn CognitiveMemoryOps>` so callers don't need to know
/// which path was taken.
pub fn connect_memory(state_root: &Path) -> SimardResult<Box<dyn CognitiveMemoryOps>> {
    let socket_path = memory_ipc::socket_path_for(state_root);
    if socket_path.exists()
        && let Ok(remote) = RemoteCognitiveMemory::connect(&socket_path)
    {
        // Wrap in SharedMemory so the trait-object type matches the
        // `Box<dyn CognitiveMemoryOps>` shape expected by OodaBridges.
        let arc: Arc<dyn CognitiveMemoryOps> = Arc::new(remote);
        let boxed: Box<dyn CognitiveMemoryOps> = Box::new(SharedMemory(arc));
        // PR-C (issue #2281, problem 3): seed bootstrap procedures
        // exactly once, post-`open`, pre-loop. Best-effort: failures
        // log and continue — daemon boot is not blocked on seeding.
        seed_bootstrap_or_log(&*boxed);
        return Ok(boxed);
    }
    let library = LibraryCognitiveMemory::open(state_root)?;
    let boxed: Box<dyn CognitiveMemoryOps> = Box::new(library);
    seed_bootstrap_or_log(&*boxed);
    Ok(boxed)
}

/// PR-C (issue #2281, problem 3): idempotent bootstrap seed wrapper
/// with logging. Calls
/// [`crate::cognitive_memory::bootstrap_procedures::seed_bootstrap_procedures`]
/// and emits one of two log lines depending on the outcome:
///
/// * `[simard] cognitive memory: N bootstrap procedures seeded`   (N > 0)
/// * `[simard] cognitive memory: 0 bootstrap procedures seeded (all present)`
/// * `[simard] cognitive memory: bootstrap seeding failed: <err>` (on error)
///
/// Seeding errors are never fatal; the daemon continues to boot.
fn seed_bootstrap_or_log(memory: &dyn CognitiveMemoryOps) {
    match crate::cognitive_memory::bootstrap_procedures::seed_bootstrap_procedures(memory) {
        Ok(0) => {
            eprintln!("[simard] cognitive memory: 0 bootstrap procedures seeded (all present)")
        }
        Ok(n) => eprintln!("[simard] cognitive memory: {n} bootstrap procedures seeded"),
        Err(e) => eprintln!("[simard] cognitive memory: bootstrap seeding failed: {e}"),
    }
}

/// Build an [`OodaBridges`] suitable for stateless helper-bin invocations.
///
/// `session` is intentionally `None`. Recipe steps that need an LLM should
/// dispatch via the `simard-engineer-loop` recipe (which spawns its own
/// session).
pub fn bridges_from_state_root(state_root: &Path) -> SimardResult<OodaBridges> {
    let memory = connect_memory(state_root)?;
    let knowledge = launch_knowledge_bridge_native()?;
    let gym = launch_gym_bridge_native()?;
    Ok(OodaBridges {
        memory,
        knowledge,
        gym,
        session: None,
        session_factory: None,
        brain: std::sync::Arc::new(crate::ooda_brain::DeterministicLifecycleBrain),
        decide_brain: None,
        orient_brain: None,
        repo_root: std::path::PathBuf::from("."),
        progress_evidence: std::sync::Arc::new(
            crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
        ),
        completion_evidence: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// De-fork Phase 2b (issue #2307): with no IPC socket present,
    /// `connect_memory` must open the LIBRARY backend directly — the native
    /// fork is deleted. The library backend persists at `<state_root>/cognitive`
    /// (a fresh LadybugDB), so its presence — together with the ABSENCE of the
    /// native fork's `<state_root>/cognitive_memory.ladybug` store — proves the
    /// library is now the sole, default backend.
    #[test]
    fn connect_memory_uses_library_backend_by_default() {
        // Use a temp state_root that has no associated IPC socket.
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        fs::create_dir_all(&state_root).unwrap();

        let mem = connect_memory(&state_root).expect("library backend open");
        drop(mem);

        assert!(
            state_root.join("cognitive").exists(),
            "library backend must create its store at <state_root>/cognitive"
        );
        assert!(
            !state_root.join("cognitive_memory.ladybug").exists(),
            "the deleted native fork store must NOT be created; library is the sole backend"
        );
    }

    /// The default library backend round-trips writes through `connect_memory`:
    /// a fact stored via the returned ops is recallable after a checkpoint +
    /// reopen of the same `state_root`, and the store lives at the library path.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn connect_memory_round_trips_through_library_backend() {
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        fs::create_dir_all(&state_root).unwrap();

        {
            let mem = connect_memory(&state_root).expect("library backend open");
            mem.store_fact("rust", "systems language", 0.9, &[], "test")
                .expect("store_fact via library backend");
            mem.checkpoint().expect("checkpoint before reopen");
        }

        let mem = connect_memory(&state_root).expect("library backend reopen");
        let facts = mem
            .search_facts("rust", 10, 0.0)
            .expect("search_facts after reopen");
        assert_eq!(
            facts.len(),
            1,
            "fact must survive reopen on the library backend"
        );
        assert_eq!(facts[0].concept, "rust");
        assert!(
            state_root.join("cognitive").exists(),
            "library backend store must persist at <state_root>/cognitive"
        );
    }
}
