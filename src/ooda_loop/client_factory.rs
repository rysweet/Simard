//! Construct an [`OodaClients`] from a `state_root` path so that recipe
//! steps (which run as short-lived helper-bin invocations) can instantiate
//! the same memories the long-lived OODA daemon uses.
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
//!    [`crate::rpc_subprocess_launcher`]. These are in-process and incur negligible
//!    startup cost compared to the former Python subprocess approach.
//! 3. Session: not constructed here. LLM sessions are heavyweight and only
//!    the long-running daemon needs one. Recipe steps that need agent
//!    delegation should use `type: recipe` to dispatch to the
//!    `simard-engineer-loop` recipe instead.
//!
//! This module is the adapter between the daemon's bespoke wiring (in
//! `operator_commands_ooda::daemon`) and the recipe-runner's stateless
//! helper-bin model. Both paths share `rpc_subprocess_launcher` for the native
//! Rust transports; they differ only in how memory and the LLM session
//! are obtained.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cognitive_memory::{
    CognitiveMemoryOps, CognitiveOpenGuard, LibraryCognitiveMemory, OpenLockOutcome,
};
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::memory_ipc::{self, RemoteCognitiveMemory, SharedMemory};
use crate::rpc_subprocess_launcher::{launch_gym_client_native, launch_knowledge_client_native};

use super::OodaClients;

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
        // `Box<dyn CognitiveMemoryOps>` shape expected by OodaClients.
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

/// Build an [`OodaClients`] suitable for stateless helper-bin invocations.
///
/// `session` is intentionally `None`. Recipe steps that need an LLM should
/// dispatch via the `simard-engineer-loop` recipe (which spawns its own
/// session).
pub fn clients_from_state_root(state_root: &Path) -> SimardResult<OodaClients> {
    // Route engineer cognition through the may-degrade resolver (R1/R2): a lost
    // cross-process open-lock race degrades to deferred/read-only cognition and
    // the engineer STILL produces its artifact, instead of the 15s fail-loud
    // hard-exit that left OODA-spawned engineers artifact-less. The degrade is
    // observed (WARN + counter) inside `connect_memory_for_role`.
    let access = connect_memory_for_role(state_root, CallerRole::Engineer)?;
    // Preserve the bootstrap-seed + logging the former `connect_memory` path did
    // here; on a deferred handle the seed writes drop harmlessly.
    seed_bootstrap_or_log(access.memory());
    let memory = access.into_memory();
    let knowledge = launch_knowledge_client_native()?;
    let gym = launch_gym_client_native()?;
    Ok(OodaClients {
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
        outcome_verify_brain: None,
        live_signals: None,
    })
}

/// Intent of a caller resolving cognitive access via [`connect_memory_for_role`].
///
/// An INTERNAL capability token: it is constructed only at trusted call sites
/// and is never derived from env / CLI / file input. It selects fail-loud vs.
/// may-degrade behaviour on a contended cross-process open — the corruption
/// guard for a genuine second writer stays intact for [`CallerRole::Daemon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallerRole {
    /// The long-lived daemon / any true exclusive writer. A contended open is a
    /// genuine second-writer signal → FAIL LOUD (preserves the lbug
    /// lock-conflict-as-corruption guard). A `Deferred` writer is unreachable for
    /// this role.
    Daemon,
    /// A short-lived OODA engineer / worktree. A contended open with no daemon
    /// IPC available → degrade to deferred/read-only cognition and keep going, so
    /// the engineer still produces its commit / PR instead of dying artifact-less.
    Engineer,
}

/// How writes behave on a resolved [`CognitiveAccess`] handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Writes persist to the store (a live library handle or the daemon IPC
    /// writer).
    Live,
    /// Writes are deferred (dropped-with-metric) and NEVER reported to the caller
    /// as persisted — the caller already knows via [`CognitiveAccess::write_mode`]
    /// that they are not persisted, so this is not a silent no-op. Reachable only
    /// for [`CallerRole::Engineer`].
    Deferred,
}

/// Resolved cognitive access handed to a caller by [`connect_memory_for_role`].
///
/// `memory()` always serves reads (shared IPC read, a live library store, or an
/// empty deferred snapshot); `write_mode()` reports whether writes persist.
pub struct CognitiveAccess {
    memory: Box<dyn CognitiveMemoryOps>,
    write_mode: WriteMode,
    degraded: bool,
}

impl std::fmt::Debug for CognitiveAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The boxed memory handle is not `Debug`; surface only the resolution
        // outcome (never store contents).
        f.debug_struct("CognitiveAccess")
            .field("write_mode", &self.write_mode)
            .field("degraded", &self.degraded)
            .finish_non_exhaustive()
    }
}

impl CognitiveAccess {
    /// The read/write handle. In [`WriteMode::Deferred`] the write methods
    /// drop-with-metric and return `Ok(..)`; reads return empty (never a lock
    /// error), so the engineer keeps reasoning.
    pub fn memory(&self) -> &dyn CognitiveMemoryOps {
        self.memory.as_ref()
    }

    /// The write mode actually granted.
    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    /// `true` iff this access degraded to deferred/read-only cognition (mirrors
    /// the emitted `simard.enrichment.degraded{reason=cognitive_open_lock}`).
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// Consume the access, yielding the underlying memory handle for placement
    /// into an [`OodaClients`] bundle. The degrade (if any) has already been
    /// observed at resolution time.
    pub(crate) fn into_memory(self) -> Box<dyn CognitiveMemoryOps> {
        self.memory
    }

    fn live(memory: Box<dyn CognitiveMemoryOps>) -> Self {
        Self {
            memory,
            write_mode: WriteMode::Live,
            degraded: false,
        }
    }
}

/// Resolve cognitive access under an explicit caller `role`.
///
/// Resolution (first success wins; no silent second exclusive open on the shared
/// root for the `Engineer` role):
///
/// 1. A daemon IPC socket present at `state_root` → the live
///    [`RemoteCognitiveMemory`] client (shared reads + serialized writes) →
///    `Live`, not degraded. This is the primary route the crash-loop error names
///    ("Route access through the daemon IPC").
/// 2. No socket, uncontended direct open → a live [`LibraryCognitiveMemory`] →
///    `Live`, not degraded.
/// 3. No socket, a CONTENDED open-lock race:
///    * [`CallerRole::Engineer`] → `WriteMode::Deferred`, `degraded = true`, plus
///      a WARN and a `simard.enrichment.degraded{reason="cognitive_open_lock"}`
///      increment (never a silent fallback).
///    * [`CallerRole::Daemon`] → `Err(SimardError::PersistentStoreIo)` (fail-loud;
///      the corruption guard is preserved).
pub fn connect_memory_for_role(
    state_root: &Path,
    role: CallerRole,
) -> SimardResult<CognitiveAccess> {
    // IPC-first for BOTH roles: share the daemon's already-open store when a
    // socket is up, so a second exclusive open never happens on the shared root.
    let socket_path = memory_ipc::socket_path_for(state_root);
    if socket_path.exists()
        && let Ok(remote) = RemoteCognitiveMemory::connect(&socket_path)
    {
        let arc: Arc<dyn CognitiveMemoryOps> = Arc::new(remote);
        return Ok(CognitiveAccess::live(Box::new(SharedMemory(arc))));
    }

    match role {
        // Unchanged fail-loud path: `LibraryCognitiveMemory::open` acquires the
        // open-lock and returns `Err(PersistentStoreIo)` on a contended store, so
        // a genuine second concurrent writer still fails loud.
        CallerRole::Daemon => Ok(CognitiveAccess::live(Box::new(
            LibraryCognitiveMemory::open(state_root)?,
        ))),
        CallerRole::Engineer => match CognitiveOpenGuard::try_acquire_classified(state_root)? {
            OpenLockOutcome::Acquired(guard) => {
                // We hold the open-lock; opening the library re-acquires it
                // re-entrantly within this process (shared via the registry), so
                // there is no window where the lock is dropped before the library
                // takes it over. Release our guard once the library holds its own.
                let library = LibraryCognitiveMemory::open(state_root)?;
                drop(guard);
                Ok(CognitiveAccess::live(Box::new(library)))
            }
            OpenLockOutcome::Contended { holder } => {
                // Fail-LOUD-but-graceful: observe the degrade (WARN + bounded
                // counter) and hand back deferred/read-only cognition. The
                // engineer proceeds and STILL produces its artifact.
                crate::enrichment_observability::observe_degrade(
                    crate::enrichment_observability::DegradeReason::CognitiveOpenLock,
                    &holder,
                );
                Ok(CognitiveAccess {
                    memory: Box::new(DeferredCognitiveMemory::new()),
                    write_mode: WriteMode::Deferred,
                    degraded: true,
                })
            }
        },
    }
}

/// Deferred/read-only cognition for an engineer that lost the open-lock race.
///
/// Reads return empty (an empty snapshot — never a lock error), so the engineer
/// keeps reasoning. Writes are **dropped-with-metric**: they return `Ok(..)`
/// (the caller already knows via [`CognitiveAccess::write_mode`] that they are
/// not persisted, so this is not a silent claim of persistence) and increment a
/// bounded in-memory counter surfaced on a DEBUG line. Nothing is buffered on
/// disk and nothing touches the contended shared store.
struct DeferredCognitiveMemory {
    /// Count of deferred (dropped) writes, for observability. Bounded — a plain
    /// counter, never an unbounded queue.
    deferred_writes: AtomicU64,
}

impl DeferredCognitiveMemory {
    fn new() -> Self {
        Self {
            deferred_writes: AtomicU64::new(0),
        }
    }

    /// Record and log one dropped write. Structured tracing only (no `println!`).
    fn drop_write(&self, op: &'static str) {
        let n = self.deferred_writes.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::debug!(
            target: "simard::enrichment",
            op = op,
            deferred_writes = n,
            "cognitive write deferred (engineer degraded to read-only cognition; not persisted)",
        );
    }
}

impl CognitiveMemoryOps for DeferredCognitiveMemory {
    fn record_sensory(
        &self,
        _modality: &str,
        _raw_data: &str,
        _ttl_seconds: u64,
    ) -> SimardResult<String> {
        self.drop_write("record_sensory");
        Ok(String::from("deferred"))
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }

    fn push_working(
        &self,
        _slot_type: &str,
        _content: &str,
        _task_id: &str,
        _relevance: f64,
    ) -> SimardResult<String> {
        self.drop_write("push_working");
        Ok(String::from("deferred"))
    }

    fn get_working(&self, _task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }

    fn clear_working(&self, _task_id: &str) -> SimardResult<usize> {
        Ok(0)
    }

    fn store_episode(
        &self,
        _content: &str,
        _source_label: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        self.drop_write("store_episode");
        Ok(String::from("deferred"))
    }

    fn consolidate_episodes(&self, _batch_size: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }

    fn store_fact(
        &self,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        self.drop_write("store_fact");
        Ok(String::from("deferred"))
    }

    fn search_facts(
        &self,
        _query: &str,
        _limit: u32,
        _min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
    }

    fn store_procedure(
        &self,
        _name: &str,
        _steps: &[String],
        _prerequisites: &[String],
    ) -> SimardResult<String> {
        self.drop_write("store_procedure");
        Ok(String::from("deferred"))
    }

    fn recall_procedure(&self, _query: &str, _limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }

    fn store_prospective(
        &self,
        _description: &str,
        _trigger_condition: &str,
        _action_on_trigger: &str,
        _priority: i64,
    ) -> SimardResult<String> {
        self.drop_write("store_prospective");
        Ok(String::from("deferred"))
    }

    fn check_triggers(&self, _content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }

    /// The deferred handle is honestly read-only: writes do not persist. This
    /// keeps a `WriterClient` from ever wrapping it as a live writer.
    fn is_read_only(&self) -> bool {
        true
    }
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
