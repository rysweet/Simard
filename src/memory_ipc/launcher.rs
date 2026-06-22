//! Cognitive-memory bridge launchers shared by dashboard, meeting, and
//! engineer call sites (issue #1590, spec recommendation C / A2).
//!
//! Two opaque types — [`WriterBridge`] and [`ReaderBridge`] — wrap a boxed
//! [`CognitiveMemoryOps`] trait object so callers can write `let bridge =
//! launch_writer_bridge(state_root)?;` and pass `bridge.ops()` straight
//! into [`crate::goal_curation::save_goal_board`] / `load_goal_board`.
//!
//! Writer ladder:
//!   0. **In-process Arc shortcut** — when the OODA daemon registered its
//!      live writer at startup via [`register_in_process_writer`] and the
//!      requested `state_root` canonicalises to the registered one,
//!      return a shared handle to the daemon's writer immediately. This
//!      is the hot path for same-process callers (dashboard, OODA loop,
//!      reflection) and bypasses IPC and disk re-open entirely.
//!   1. Connect to the running OODA daemon's UDS at
//!      [`super::socket_path_for`] when present and the state_root
//!      matches — used by separate-process clients (meeting REPL, engineer
//!      subprocesses).
//!   2. Reap any stale open-lock left by a crashed prior process and
//!      [`LibraryCognitiveMemory::open`] the store directly.
//!
//! There is **no** silent read-only fallback. If the launcher cannot
//! produce a writer that can actually write, it returns `Err`. The
//! earlier "tier 3 = open_read_only" path was the root cause of the
//! dashboard "hollow success" bug — `{"status":"ok"}` responses with no
//! persisted change. See issue #1590 follow-up.
//!
//! Reader semantics: prefer the in-process Arc, then the daemon socket,
//! then a direct [`LibraryCognitiveMemory::open`] (which creates the store
//! if the underlying DB has never been opened).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::{SimardError, SimardResult};

use super::{RemoteCognitiveMemory, SharedMemory, reap_stale_open_lock, socket_path_for};

/// Writer bridge to cognitive memory. Holds a `Box<dyn CognitiveMemoryOps>`
/// underneath; callers should use [`WriterBridge::ops`] to access it.
pub struct WriterBridge {
    /// `Option` so [`WriterBridge::into_box`] can move the inner handle out
    /// even though this type has a `Drop` impl.
    inner: Option<Box<dyn CognitiveMemoryOps>>,
    /// When `true`, [`Drop`] checkpoints the handle so its writes reach the
    /// main DB file. Set only for the direct-open cached handle (issue #2320):
    /// because that handle is cached process-wide and never dropped, the
    /// per-call `Database::drop` that used to checkpoint on the direct-open
    /// path no longer runs, so the bridge checkpoints on drop instead. The
    /// daemon (in-process Arc) and IPC bridges leave this `false` — durability
    /// there is owned by the daemon's lifecycle / a no-op over IPC.
    checkpoint_on_drop: bool,
}

impl WriterBridge {
    /// Construct a writer bridge, asserting the wrapped backend is not
    /// read-only.
    ///
    /// Wrapping a read-only handle as a `WriterBridge` is exactly the
    /// silent-degradation hazard the issue #1590 follow-up eliminates:
    /// `store_fact` returning `Ok(())` against a read-only backend
    /// produces "hollow success" responses (e.g. dashboard
    /// `{"status":"ok"}` with no change visible on the next read).
    /// Construction panics rather than silently succeeds — this is a
    /// programming error, not a runtime condition the caller can
    /// recover from.
    fn checked_new(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        assert!(
            !inner.is_read_only(),
            "WriterBridge: refusing to wrap a read-only handle (silent-degradation hazard — \
             writes against this bridge would no-op without surfacing an error)"
        );
        Self {
            inner: Some(inner),
            checkpoint_on_drop: false,
        }
    }

    /// Like [`Self::checked_new`] but checkpoints the handle on drop. Used by
    /// the direct-open cached path so writes are durable across sessions even
    /// though the cached handle itself never drops (issue #2320).
    fn checked_new_checkpointing(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        let mut bridge = Self::checked_new(inner);
        bridge.checkpoint_on_drop = true;
        bridge
    }

    /// Borrow the underlying ops object so it can be passed to
    /// `save_goal_board` / `load_goal_board` / etc.
    pub fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.inner
            .as_deref()
            .expect("WriterBridge inner present until into_box")
    }

    /// Consume the bridge and return the underlying boxed ops. Used by
    /// legacy call sites (e.g. `launch_real_meeting_bridge`) that hold a
    /// `Box<dyn CognitiveMemoryOps>` directly.
    pub fn into_box(mut self) -> Box<dyn CognitiveMemoryOps> {
        self.inner
            .take()
            .expect("WriterBridge inner present until into_box")
    }

    /// Test-only constructor that pins the read-only invariant. Panics
    /// under the same conditions as the internal `checked_new`.
    #[cfg(test)]
    pub fn from_ops_for_test(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        Self::checked_new(inner)
    }
}

impl Drop for WriterBridge {
    fn drop(&mut self) {
        if self.checkpoint_on_drop
            && let Some(inner) = self.inner.as_deref()
        {
            // Best-effort: a failed checkpoint must not panic in Drop. The
            // write already succeeded; the worst case is the next session
            // replays the WAL.
            let _ = inner.checkpoint();
        }
    }
}

/// Reader bridge to cognitive memory. Either the daemon's IPC client (which
/// serializes through the daemon) or a direct [`LibraryCognitiveMemory::open`].
pub struct ReaderBridge {
    inner: Box<dyn CognitiveMemoryOps>,
}

impl ReaderBridge {
    pub fn ops(&self) -> &dyn CognitiveMemoryOps {
        &*self.inner
    }
}

// ---------------------------------------------------------------------------
// Tier 0: in-process Arc shortcut.
//
// The OODA daemon owns one writer per process. At startup it registers
// that writer (along with the state_root it was opened against) here.
// Same-process callers — dashboard handler, reflection loop, etc. — that
// ask `launch_writer_bridge(state_root)` for the same state_root receive
// a shared handle to the daemon's writer immediately, bypassing IPC and
// the direct-open ladder.
//
// The registration is path-aware: only requests whose `state_root`
// canonicalises to the registered one short-circuit. This protects tests
// that pass arbitrary temp-dir state_roots from accidentally receiving
// the daemon's writer.
//
// IMPORTANT (issue #1590): the registration stores a `Weak` reference,
// not a strong `Arc`. Rust does NOT drop `static` items at process
// exit, so a strong `Arc` here would prevent the inner `lbug::Database`
// from ever dropping. lbug's `force_checkpoint_on_close` only fires on
// `Database::drop` — keeping the strong Arc here would cause writes to
// stay buffered in the WAL forever and never reach the main DB file.
// Using `Weak` lets the registration coexist with the daemon's (or the
// bootstrap's) own strong Arc; when that strong Arc drops at process
// exit, the Database drops and checkpoints. Subsequent processes
// opening the DB read-only then see the committed writes.
// ---------------------------------------------------------------------------

static IN_PROCESS_WRITER: RwLock<Option<(PathBuf, Weak<dyn CognitiveMemoryOps>)>> =
    RwLock::new(None);

fn canonical_or_self(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Register an in-process writer that [`launch_writer_bridge`] should
/// return immediately when called with the same `state_root`.
///
/// The OODA daemon calls this at startup with its live
/// `Arc<dyn CognitiveMemoryOps>` (the same handle that backs the IPC
/// server). After registration, in-process callers (the dashboard,
/// reflection loop, …) skip the IPC round-trip and the direct-open
/// ladder entirely — they share the daemon's writer through `Arc::clone`.
///
/// The registration stores a `Weak` reference; the caller must keep
/// the strong `Arc` alive for as long as the registration is meant to
/// be valid. When the strong Arc is dropped (e.g. at process exit),
/// the registration silently expires — `lookup_in_process_writer`
/// returns `None` and the launcher falls through to the next ladder
/// tier. This avoids the static-Arc-leak that prevented
/// `lbug::Database::drop` from running at process exit and stranded
/// writes in the WAL (issue #1590).
///
/// Subsequent calls overwrite the previous registration (last writer
/// wins). In production there is exactly one daemon writer per process,
/// so overwriting is harmless. Tests that need to reset the registration
/// can call [`clear_in_process_writer`].
pub fn register_in_process_writer(state_root: PathBuf, writer: Arc<dyn CognitiveMemoryOps>) {
    let key = canonical_or_self(&state_root);
    if let Ok(mut g) = IN_PROCESS_WRITER.write() {
        *g = Some((key, Arc::downgrade(&writer)));
    }
}

/// Clear any registered in-process writer.
///
/// **Shutdown-only — do not call from request paths.** This drops the
/// `Weak` reference so the next [`launch_writer_bridge`] call falls
/// through to the IPC/disk ladder. The OODA daemon's signal-driven
/// shutdown sequence calls this immediately after `persist_board` so
/// the writer Arc can drop deterministically before
/// `Database::drop` runs `force_checkpoint_on_close` (issue #1631).
///
/// Tests also call this between runs to reset global state.
pub fn clear_in_process_writer() {
    if let Ok(mut g) = IN_PROCESS_WRITER.write() {
        *g = None;
    }
}

/// Look up a registered in-process writer for `state_root`. Returns
/// `Some(arc)` only if both `state_root` and the registered key
/// canonicalise to the same path AND the registered `Weak` still
/// upgrades to a live strong `Arc`.
fn lookup_in_process_writer(state_root: &Path) -> Option<Arc<dyn CognitiveMemoryOps>> {
    let g = IN_PROCESS_WRITER.read().ok()?;
    let (registered_root, weak) = g.as_ref()?;
    if canonical_or_self(state_root) != canonical_or_self(registered_root) {
        return None;
    }
    weak.upgrade()
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Tier 2 shared store cache.
//
// When neither the daemon's in-process writer (tier 0) nor its IPC socket
// (tier 1) is available, the launcher opens the library store directly. The
// naive implementation opened a *fresh* `LibraryCognitiveMemory` on every
// `launch_writer_bridge` / `open_reader_bridge` call, so a sequence of
// open→write→drop→reopen→read cycles against the same `state_root` within one
// process (e.g. the dashboard goal-CRUD handlers, or any same-process
// read-after-write) reopened the lbug `Database` repeatedly.
//
// That reopen is racy: a fresh open intermittently returns fact rows whose
// per-fact metadata (the adapter's `_simard_seq` ordering key) has not yet been
// folded back in, which collapses the "max node_id == newest" invariant the
// goal-board snapshot read depends on and surfaces as an empty / stale read
// (issue #2320 — flaky `full_goal_lifecycle_crud`).
//
// Fix: cache one shared `Arc<LibraryCognitiveMemory>` per canonical
// `state_root` and hand every tier-2 reader/writer a `SharedMemory` view of it.
// Reads and writes then go through a single in-memory store, so a write is
// immediately visible to the next read with no reopen and no metadata-loss
// race — the same single-handle guarantee the daemon already gets from tier 0.
//
// Scope of the guarantee: this provides **same-process** read-after-write
// consistency for tier-2 callers. Cross-process visibility is unchanged — it
// flows through the daemon IPC socket (tier 1) or, for a cold open in a later
// process, the library's WAL replay. This cache deliberately makes tier-2 a
// process-local single-owner of each `state_root`; a caller needing a fresh
// on-disk view from another writer must go through the daemon socket.
//
// Lifetime / memory: the cache holds a strong `Arc`, so the handle survives
// across the short-lived bridges that come and go between operations (that
// persistence is the whole point). To bound growth — the test suite allocates a
// fresh `TempDir` state_root per hermetic test — a lookup whose directory has
// been removed falls through to the slow path, which prunes every entry whose
// directory no longer exists. A `TempDir` is removed when its owning test
// drops, so its cached handle is evicted on the next mismatching access;
// dropping the last `Arc` runs `lbug::Database::drop`, which checkpoints the
// WAL into the main file. [`HermeticState::drop`] additionally evicts a test's
// own entry eagerly via [`evict_cached_direct_handle`] so the handle is closed
// (and checkpointed) before its `TempDir` is reaped (issue #2320). Live state
// roots (the daemon is on tier 0, so this is CLI / embedded callers) stay
// cached for the process lifetime; the library replays any un-checkpointed WAL
// on the next open, and writer bridges checkpoint the shared handle on drop
// (see [`WriterBridge::checked_new_checkpointing`]) so a just-written snapshot
// is durable across sessions even though the cached handle itself never drops.
// [`clear_tier2_store_cache`] drops every handle deterministically (flushing via
// `Database::drop`) for shutdown/tests.
// ---------------------------------------------------------------------------

static TIER2_STORE_CACHE: RwLock<BTreeMap<PathBuf, Arc<LibraryCognitiveMemory>>> =
    RwLock::new(BTreeMap::new());

/// Drop cache entries whose canonical directory no longer exists. Called while
/// holding the write lock. Returns nothing; eviction is best-effort and
/// dropping the evicted `Arc`s checkpoints their stores via `Database::drop`.
fn prune_dead_store_entries(cache: &mut BTreeMap<PathBuf, Arc<LibraryCognitiveMemory>>) {
    cache.retain(|path, _| path.exists());
}

/// Get-or-open the shared library store for `state_root`, eliminating the
/// per-operation reopen race (issue #2320). All tier-2 readers and writers for
/// the same canonical `state_root` share this one handle.
fn shared_tier2_store(state_root: &Path) -> SimardResult<Arc<LibraryCognitiveMemory>> {
    // Materialise the directory before keying so the canonical path is stable
    // regardless of whether the writer (which already `create_dir_all`s) or the
    // reader reaches here first. Without this, a reader-first call on a not-yet-
    // existing symlinked temp root would key under the raw path while a later
    // writer keys under the resolved path — two keys, two live handles on one
    // on-disk DB, which is exactly the double-open `SharedMemory` exists to avoid.
    let _ = std::fs::create_dir_all(state_root);
    let key = canonical_or_self(state_root);

    // Fast path: already cached AND the backing directory still exists. The
    // existence check guards the (test-only) case where a state_root dir is
    // removed and a fresh dir is later created at the same path — we must not
    // hand back the stale handle, so fall through to the pruning slow path.
    if let Ok(cache) = TIER2_STORE_CACHE.read()
        && let Some(arc) = cache.get(&key)
        && key.exists()
    {
        return Ok(Arc::clone(arc));
    }

    // Slow path: open under the write lock so two callers racing on the same
    // path don't both open the lbug `Database` (which would contend on the
    // on-disk store lock). Recover from a poisoned lock rather than failing —
    // the map itself is not left in an inconsistent state by a panicking
    // holder.
    let mut cache = TIER2_STORE_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_dead_store_entries(&mut cache);
    if let Some(arc) = cache.get(&key) {
        return Ok(Arc::clone(arc));
    }
    let mem = Arc::new(LibraryCognitiveMemory::open(state_root)?);
    cache.insert(key, Arc::clone(&mem));
    Ok(mem)
}

/// Drop every cached tier-2 store handle, flushing each via `Database::drop`
/// (which checkpoints the WAL into the main file).
///
/// Shutdown / test helper: production code rarely needs this because the daemon
/// path uses tier 0 and per-`state_root` handles are pruned as their temp dirs
/// disappear. Call it at process shutdown for a deterministic checkpoint, or in
/// a test that must force a genuine cold reopen of a still-live `state_root`.
pub fn clear_tier2_store_cache() {
    let mut cache = TIER2_STORE_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.clear();
}

/// Evict (and drop) the cached tier-2 store handle for `state_root`, if present.
///
/// Called by `HermeticState::drop` so a test's cached lbug handle is closed —
/// dropping the last `Arc` runs `Database::drop`, which checkpoints the WAL into
/// the main file — before its `TempDir` is reaped. Otherwise the handle would
/// keep the store open on a soon-to-be-deleted directory and the existence-based
/// prune would only release it on a later mismatching access. A no-op when
/// nothing is cached for `state_root` (issue #2320).
pub fn evict_cached_direct_handle(state_root: &Path) {
    let key = canonical_or_self(state_root);
    let mut cache = TIER2_STORE_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.remove(&key);
}

/// Launch a cognitive-memory writer bridge against `state_root`.
///
/// Resolution ladder:
///   0. In-process Arc shortcut.
///   1. IPC to the socket resolved by [`socket_path_for(state_root)`].
///      The socket follows the requested `state_root`, so a hermetic
///      TempDir reader/writer never collides with the live daemon's
///      socket at `~/.simard/state/memory.sock` (closes
///      [#1923](https://github.com/rysweet/Simard/issues/1923) /
///      [#1925](https://github.com/rysweet/Simard/issues/1925)).
///   2. Reap any stale lock and `LibraryCognitiveMemory::open` directly.
///
/// **No read-only fallback.** A writer bridge that cannot write is a
/// silent-degradation hazard (the dashboard hollow-success bug from
/// issue #1590); if no tier yields a writer, the launcher returns `Err`.
pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge> {
    #[cfg(test)]
    crate::test_support::hermetic_guard::assert_state_root_isolated(
        state_root,
        "launch_writer_bridge",
    );

    let _ = std::fs::create_dir_all(state_root);

    // (0) In-process Arc shortcut — same-process callers sharing the
    // daemon's writer.
    if let Some(arc) = lookup_in_process_writer(state_root) {
        return Ok(WriterBridge::checked_new(Box::new(SharedMemory(arc))));
    }

    // (1) Prefer the running daemon's IPC writer when the resolved socket
    // for `state_root` is up. `socket_path_for` returns
    // `<state_root>/memory.sock` by default, so a TempDir state root can
    // never collide with the live daemon's socket at
    // `~/.simard/state/memory.sock` — this is what makes
    // `SIMARD_STATE_ROOT` actually hermetic (#1923/#1925). The
    // `SIMARD_MEMORY_SOCKET` env var still overrides for the rare cross-
    // mount probe.
    let sock = socket_path_for(state_root);
    if sock.exists() {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                return Ok(WriterBridge::checked_new(Box::new(client)));
            }
            Err(e) => {
                eprintln!(
                    "[simard] launch_writer_bridge: socket {} present but connect failed \
                     ({e}); falling back to direct open",
                    sock.display()
                );
            }
        }
    }

    // (2) No daemon — reap any stale lock and open the shared tier-2 store.
    // The store is cached per canonical `state_root` (see `shared_tier2_store`)
    // so every same-process reader/writer for this root shares one handle,
    // eliminating the open→write→drop→reopen→read metadata-loss race that made
    // `full_goal_lifecycle_crud` flaky (issue #2320).
    if let Err(e) = reap_stale_open_lock(state_root) {
        eprintln!("[simard] launch_writer_bridge: stale-lock reap failed: {e}");
    }

    let mem = shared_tier2_store(state_root).map_err(|e| SimardError::RuntimeInitFailed {
        component: "memory-ipc-launcher".into(),
        reason: format!(
            "cognitive memory writer unavailable at {} — IPC and direct open both failed: \
             {e}. Read-only fallback is disabled because writes against a read-only handle \
             silently no-op (issue #1590).",
            state_root.display()
        ),
    })?;
    // Checkpoint the shared handle on drop: it is cached process-wide and never
    // drops, so the per-call `Database::drop` that would flush the WAL no longer
    // runs. The bridge checkpoints explicitly so a CLI/meeting/engineer/goal-store
    // write is durable across sessions (issue #2320).
    Ok(WriterBridge::checked_new_checkpointing(Box::new(
        SharedMemory(mem as Arc<dyn CognitiveMemoryOps>),
    )))
}

/// Open a cognitive-memory reader bridge against `state_root`.
///
/// Resolution ladder:
///   0. In-process Arc shortcut.
///   1. Try `RemoteCognitiveMemory::connect(socket_path_for(state_root))`.
///   2. Otherwise a direct [`LibraryCognitiveMemory::open`] — creates the
///      store if the DB has never been opened.
pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge> {
    // (0) Same-process daemon writer: serves reads too.
    if let Some(arc) = lookup_in_process_writer(state_root) {
        return Ok(ReaderBridge {
            inner: Box::new(SharedMemory(arc)),
        });
    }

    // (1) Prefer the daemon socket when present — the socket follows the
    // requested state_root via `socket_path_for`, so this naturally
    // routes a hermetic TempDir reader to its own DB even when the live
    // daemon is up on `~/.simard/state/memory.sock`.
    let sock = socket_path_for(state_root);
    if sock.exists() {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                return Ok(ReaderBridge {
                    inner: Box::new(client),
                });
            }
            Err(e) => {
                eprintln!(
                    "[simard] open_reader_bridge: socket {} present but connect failed ({e}); \
                     falling back to direct open",
                    sock.display()
                );
            }
        }
    }

    // (2) De-fork Phase 2b (issue #2307): direct open of the library-backed
    // store, shared per canonical `state_root` (see `shared_tier2_store`). The
    // library has no read-only constructor, so this is a writer handle used for
    // reads — acceptable because tiers 0/1 already cover the case where the
    // daemon holds the store. Sharing the handle with the tier-2 writer makes a
    // just-written goal-board snapshot immediately visible here instead of
    // racing a fresh reopen (issue #2320).
    let mem = shared_tier2_store(state_root)?;
    Ok(ReaderBridge {
        inner: Box::new(SharedMemory(mem as Arc<dyn CognitiveMemoryOps>)),
    })
}
