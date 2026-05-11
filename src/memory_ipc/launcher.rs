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
//!      [`super::default_socket_path`] when present and the state_root
//!      matches — used by separate-process clients (meeting REPL, engineer
//!      subprocesses).
//!   2. Reap any stale open-lock left by a crashed prior process and
//!      [`NativeCognitiveMemory::open`] the DB directly.
//!
//! There is **no** silent read-only fallback. If the launcher cannot
//! produce a writer that can actually write, it returns `Err`. The
//! earlier "tier 3 = open_read_only" path was the root cause of the
//! dashboard "hollow success" bug — `{"status":"ok"}` responses with no
//! persisted change. See issue #1590 follow-up.
//!
//! Reader semantics: prefer the in-process Arc, then the daemon socket,
//! then [`NativeCognitiveMemory::open_read_only`] (which fails when the
//! underlying DB has never been opened).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::{SimardError, SimardResult};

use super::{
    RemoteCognitiveMemory, SharedMemory, default_socket_path, default_state_root,
    reap_stale_open_lock,
};

/// Writer bridge to cognitive memory. Holds a `Box<dyn CognitiveMemoryOps>`
/// underneath; callers should use [`WriterBridge::ops`] to access it.
pub struct WriterBridge {
    inner: Box<dyn CognitiveMemoryOps>,
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
        Self { inner }
    }

    /// Borrow the underlying ops object so it can be passed to
    /// `save_goal_board` / `load_goal_board` / etc.
    pub fn ops(&self) -> &dyn CognitiveMemoryOps {
        &*self.inner
    }

    /// Consume the bridge and return the underlying boxed ops. Used by
    /// legacy call sites (e.g. `launch_real_meeting_bridge`) that hold a
    /// `Box<dyn CognitiveMemoryOps>` directly.
    pub fn into_box(self) -> Box<dyn CognitiveMemoryOps> {
        self.inner
    }

    /// Test-only constructor that pins the read-only invariant. Panics
    /// under the same conditions as the internal `checked_new`.
    #[cfg(test)]
    pub fn from_ops_for_test(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        Self::checked_new(inner)
    }
}

/// Reader bridge to cognitive memory. Read-only by construction (either the
/// daemon's IPC client, which serializes through the daemon, or
/// [`NativeCognitiveMemory::open_read_only`]).
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
/// Called during graceful daemon shutdown so the registered `Weak` is
/// dropped before the strong `Arc` it points to. Tests also use it to
/// reset state across runs.
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

/// Public alias of `lookup_in_process_writer` used by the backup helper
/// (in another module) to ask the live writer to checkpoint before the
/// backup file is copied. The `_for_test` suffix is historical — this is
/// production code; tests just happen to exercise the same codepath.
pub fn lookup_in_process_writer_for_test(state_root: &Path) -> Option<Arc<dyn CognitiveMemoryOps>> {
    lookup_in_process_writer(state_root)
}

/// Decide whether `state_root` matches the daemon's owned state root.
/// Only when they agree should a launcher route through the daemon's IPC
/// socket — otherwise we'd be talking to a daemon that owns a different
/// DB.
fn state_root_matches_daemon(state_root: &Path) -> bool {
    let daemon_root = default_state_root();
    match (state_root.canonicalize(), daemon_root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Launch a cognitive-memory writer bridge against `state_root`.
///
/// Resolution ladder:
///   0. In-process Arc shortcut.
///   1. IPC to the daemon's Unix socket when present and matched.
///   2. Reap any stale lock and `NativeCognitiveMemory::open` directly.
///
/// **No read-only fallback.** A writer bridge that cannot write is a
/// silent-degradation hazard (the dashboard hollow-success bug from
/// issue #1590); if no tier yields a writer, the launcher returns `Err`.
pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge> {
    let _ = std::fs::create_dir_all(state_root);

    // (0) In-process Arc shortcut — same-process callers sharing the
    // daemon's writer.
    if let Some(arc) = lookup_in_process_writer(state_root) {
        return Ok(WriterBridge::checked_new(Box::new(SharedMemory(arc))));
    }

    // (1) Prefer the running daemon's IPC writer — but only when our
    // requested state_root actually matches the daemon's.
    let sock = default_socket_path();
    if sock.exists() && state_root_matches_daemon(state_root) {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                return Ok(WriterBridge::checked_new(Box::new(client)));
            }
            Err(e) => {
                eprintln!(
                    "[simard] launch_writer_bridge: daemon socket present but connect failed \
                     ({e}); falling back to direct open"
                );
            }
        }
    }

    // (2) No daemon — reap any stale lock and try direct open.
    if let Err(e) = reap_stale_open_lock(state_root) {
        eprintln!("[simard] launch_writer_bridge: stale-lock reap failed: {e}");
    }

    let mem =
        NativeCognitiveMemory::open(state_root).map_err(|e| SimardError::RuntimeInitFailed {
            component: "memory-ipc-launcher".into(),
            reason: format!(
                "cognitive memory writer unavailable at {} — IPC and direct open both failed: \
                 {e}. Read-only fallback is disabled because writes against a read-only handle \
                 silently no-op (issue #1590).",
                state_root.display()
            ),
        })?;
    Ok(WriterBridge::checked_new(Box::new(mem)))
}

/// Open a cognitive-memory reader bridge against `state_root`.
///
/// Resolution ladder:
///   0. In-process Arc shortcut.
///   1. Try `RemoteCognitiveMemory::connect(default_socket_path())`.
///   2. Otherwise `NativeCognitiveMemory::open_read_only` — fails when
///      the DB has never been opened.
pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge> {
    // (0) Same-process daemon writer: serves reads too.
    if let Some(arc) = lookup_in_process_writer(state_root) {
        return Ok(ReaderBridge {
            inner: Box::new(SharedMemory(arc)),
        });
    }

    // (1) Prefer the daemon socket when present — but only when the
    // requested state_root matches the daemon's.
    let sock = default_socket_path();
    if sock.exists() && state_root_matches_daemon(state_root) {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                return Ok(ReaderBridge {
                    inner: Box::new(client),
                });
            }
            Err(e) => {
                eprintln!(
                    "[simard] open_reader_bridge: daemon socket present but connect failed ({e}); \
                     falling back to direct open_read_only"
                );
            }
        }
    }

    // (2) Direct read-only open of the on-disk DB.
    let mem = NativeCognitiveMemory::open_read_only(state_root)?;
    Ok(ReaderBridge {
        inner: Box::new(mem),
    })
}
