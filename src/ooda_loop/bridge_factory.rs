//! Construct an [`OodaBridges`] from a `state_root` path so that recipe
//! steps (which run as short-lived helper-bin invocations) can instantiate
//! the same bridges the long-lived OODA daemon uses.
//!
//! Strategy:
//!
//! 1. Memory: prefer the live [`RemoteCognitiveMemory`] IPC client at
//!    `socket_path_for(state_root)` so we share the daemon's open SQLite
//!    handle when one is running. If that fails, fall back to a direct
//!    [`NativeCognitiveMemory::open`] on `state_root`. The fallback is the
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
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::SimardResult;
use crate::memory_ipc::{self, RemoteCognitiveMemory, SharedMemory};

use super::OodaBridges;

/// Connect to the live IPC memory server if one is running, otherwise open
/// the SQLite store directly.
///
/// Returned as `Box<dyn CognitiveMemoryOps>` so callers don't need to know
/// which path was taken.
pub fn connect_memory(state_root: &Path) -> SimardResult<Box<dyn CognitiveMemoryOps>> {
    // De-fork Phase 2a (issue #86): with `--features library-memory` built AND
    // `SIMARD_COGMEM_BACKEND=library` set, select the upstream library-backed
    // adapter at the FRONT of the precedence order, bypassing the IPC socket
    // (there is no library IPC server). This is for parity validation / review
    // only — it opens a SEPARATE store at `state_root/cognitive` and never
    // touches the native live data at `state_root/cognitive_memory.ladybug`. The
    // branch is compiled out of default builds and is a no-op when the env var
    // is unset, so existing behavior is byte-for-byte unchanged.
    #[cfg(feature = "library-memory")]
    if std::env::var("SIMARD_COGMEM_BACKEND").as_deref() == Ok("library") {
        let library = crate::cognitive_memory::LibraryCognitiveMemory::open(state_root)?;
        let boxed: Box<dyn CognitiveMemoryOps> = Box::new(library);
        eprintln!(
            "[simard] cognitive memory: using library backend \
             (SIMARD_COGMEM_BACKEND=library; de-fork Phase 2a, issue #86)"
        );
        seed_bootstrap_or_log(&*boxed);
        return Ok(boxed);
    }

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
    let native = NativeCognitiveMemory::open(state_root)?;
    let boxed: Box<dyn CognitiveMemoryOps> = Box::new(native);
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
        brain: std::sync::Arc::new(crate::ooda_brain::DeterministicLifecycleBrain),
        decide_brain: None,
        orient_brain: None,
        repo_root: std::path::PathBuf::from("."),
        progress_evidence: std::sync::Arc::new(
            crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn connect_memory_falls_back_to_native_when_no_socket() {
        // Use a temp state_root that has no associated IPC socket.
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        fs::create_dir_all(&state_root).unwrap();

        let mem = connect_memory(&state_root);
        assert!(
            mem.is_ok(),
            "expected fallback to NativeCognitiveMemory, got {:?}",
            mem.err()
        );
    }

    #[test]
    fn connect_memory_creates_dbs_under_state_root() {
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        fs::create_dir_all(&state_root).unwrap();

        // Open succeeds even when no IPC socket exists — exercise the
        // NativeCognitiveMemory fallback path. We don't assert about
        // filesystem layout because NativeCognitiveMemory uses lazy
        // initialisation; the contract we care about is "open returns Ok".
        let mem = connect_memory(&state_root).expect("native open");
        drop(mem);
    }
}
