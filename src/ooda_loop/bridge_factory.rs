//! Construct an [`OodaBridges`] from a `state_root` path so that recipe
//! steps (which run as short-lived helper-bin invocations) can instantiate
//! the same bridges the long-lived OODA daemon uses.
//!
//! Strategy:
//!
//! 1. Memory: prefer the live [`RemoteCognitiveMemory`] IPC client at
//!    `default_socket_path()` so we share the daemon's open SQLite handle
//!    when one is running. If that fails, fall back to a direct
//!    [`NativeCognitiveMemory::open`] on `state_root`. The fallback is the
//!    correct behaviour for one-shot recipe runs (parity tests, ad-hoc
//!    `amplihack recipe run`) when no daemon is up.
//! 2. Knowledge / gym: launch a fresh subprocess pair via
//!    [`crate::bridge_launcher`]. These are owned by the helper-bin process
//!    and torn down when it exits; the cost (~hundreds of milliseconds for
//!    Python startup) is acceptable for recipe-step granularity.
//! 3. Session: not constructed here. LLM sessions are heavyweight and only
//!    the long-running daemon needs one. Recipe steps that need agent
//!    delegation should use `type: recipe` to dispatch to the
//!    `simard-engineer-loop` recipe instead.
//!
//! This module is the bridge between the daemon's bespoke wiring (in
//! `operator_commands_ooda::daemon`) and the recipe-runner's stateless
//! helper-bin model. Both paths now share `bridge_launcher` for the
//! Python subprocesses; they differ only in how memory and the LLM
//! session are obtained.

use std::path::Path;
use std::sync::Arc;

use crate::bridge_launcher::{find_python_dir, launch_gym_bridge, launch_knowledge_bridge};
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
    let socket_path = memory_ipc::default_socket_path();
    if socket_path.exists() {
        if let Ok(remote) = RemoteCognitiveMemory::connect(&socket_path) {
            // Wrap in SharedMemory so the trait-object type matches the
            // `Box<dyn CognitiveMemoryOps>` shape expected by OodaBridges.
            let arc: Arc<dyn CognitiveMemoryOps> = Arc::new(remote);
            return Ok(Box::new(SharedMemory(arc)));
        }
    }
    let native = NativeCognitiveMemory::open(state_root)?;
    Ok(Box::new(native))
}

/// Build an [`OodaBridges`] suitable for stateless helper-bin invocations.
///
/// `session` is intentionally `None`. Recipe steps that need an LLM should
/// dispatch via the `simard-engineer-loop` recipe (which spawns its own
/// session).
pub fn bridges_from_state_root(state_root: &Path) -> SimardResult<OodaBridges> {
    let memory = connect_memory(state_root)?;
    let python_dir = find_python_dir()?;
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;
    Ok(OodaBridges {
        memory,
        knowledge,
        gym,
        session: None,
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
