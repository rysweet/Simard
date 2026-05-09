//! Cognitive-memory bridge launchers shared by dashboard, meeting, and
//! engineer call sites (issue #1590, spec recommendation C / A2).
//!
//! Two opaque types — [`WriterBridge`] and [`ReaderBridge`] — wrap a boxed
//! [`CognitiveMemoryOps`] trait object so callers can write `let bridge =
//! launch_writer_bridge(state_root)?;` and pass `bridge.ops()` straight
//! into [`crate::goal_curation::save_goal_board`] / `load_goal_board`.
//!
//! The writer ladder mirrors `launch_real_meeting_bridge`:
//!   1. Connect to the running OODA daemon's UDS at
//!      [`super::default_socket_path`] when present (shared writer, no
//!      lock contention).
//!   2. Reap any stale open-lock left by a crashed prior process and
//!      [`NativeCognitiveMemory::open`] the DB directly.
//!   3. Last-resort: open read-only — surfaced as `Ok` here because
//!      callers may legitimately fall through to a read-only behavior;
//!      write attempts will surface their own errors at call time.
//!
//! Reader semantics: prefer the daemon socket; otherwise
//! [`NativeCognitiveMemory::open_read_only`], which fails when the
//! underlying DB has never been opened.

use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::{SimardError, SimardResult};

use super::{
    RemoteCognitiveMemory, SharedMemory, default_socket_path, default_state_root,
    reap_stale_open_lock,
};

/// Process-wide registration slot for the in-process cognitive-memory
/// writer. The OODA daemon registers its `Arc<dyn CognitiveMemoryOps>`
/// here at startup; subsequent same-process callers (dashboard,
/// reflection loop, goal store) skip IPC and direct-open entirely and
/// share the daemon's writer through `Arc::clone`.
///
/// Production: only the first registration wins; subsequent calls are
/// silently ignored. Tests can use [`unregister_in_process_writer_for_test`]
/// to reset between cases — combine with `#[serial]` from `serial_test` to
/// avoid cross-test pollution.
fn in_process_writer_slot() -> &'static RwLock<Option<Arc<dyn CognitiveMemoryOps>>> {
    static SLOT: std::sync::OnceLock<RwLock<Option<Arc<dyn CognitiveMemoryOps>>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

fn current_in_process_writer() -> Option<Arc<dyn CognitiveMemoryOps>> {
    in_process_writer_slot()
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone))
}

/// Writer bridge to cognitive memory. Holds a `Box<dyn CognitiveMemoryOps>`
/// underneath; callers should use [`WriterBridge::ops`] to access it.
pub struct WriterBridge {
    inner: Box<dyn CognitiveMemoryOps>,
}

impl WriterBridge {
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

    /// Defensive constructor: panics if the wrapped handle reports
    /// `is_read_only() == true`. A `WriterBridge` wrapping a read-only
    /// handle is exactly the silent-degradation hazard issue #1590
    /// targets — write attempts would silently no-op or surface
    /// `BridgeTransportError` only after the `Ok(())` path has been
    /// threaded through dashboards / probes.
    fn checked_new(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        assert!(
            !inner.is_read_only(),
            "WriterBridge cannot wrap a read-only handle — would silently swallow writes"
        );
        Self { inner }
    }

    /// Test-only constructor that mirrors [`Self::checked_new`].
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

/// Register an in-process writer that [`launch_writer_bridge`] should
/// return immediately when called from the same process.
///
/// The OODA daemon calls this at startup with its live
/// `Arc<dyn CognitiveMemoryOps>` (the same handle that backs the IPC
/// server). After registration, in-process callers (the dashboard,
/// reflection loop, …) skip the IPC round-trip and the direct-open
/// ladder entirely — they share the daemon's writer through `Arc::clone`.
///
/// Only the first call wins; subsequent calls are silently ignored. This
/// is sufficient because there is exactly one daemon writer per process.
pub fn register_in_process_writer(writer: Arc<dyn CognitiveMemoryOps>) {
    if let Ok(mut guard) = in_process_writer_slot().write()
        && guard.is_none()
    {
        *guard = Some(writer);
    }
}

/// Test-only: clear any registered in-process writer so subsequent calls
/// to [`launch_writer_bridge`] / [`open_reader_bridge`] fall through to
/// the IPC and direct-open tiers.
///
/// Combine with `#[serial_test::serial]` to avoid pollution from tests
/// that need fresh registration semantics.
#[cfg(test)]
pub(crate) fn unregister_in_process_writer_for_test() {
    if let Ok(mut guard) = in_process_writer_slot().write() {
        *guard = None;
    }
}

/// Decide whether `state_root` matches the daemon's owned state root.
/// Only when they agree should a launcher route through the daemon's IPC
/// socket — otherwise we'd be talking to a daemon that owns a different DB.
fn state_root_matches_daemon(state_root: &Path) -> bool {
    let daemon_root = default_state_root();
    match (state_root.canonicalize(), daemon_root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        // If the daemon's root doesn't exist on disk, neither side can match.
        _ => false,
    }
}

/// Launch a cognitive-memory writer bridge against `state_root`.
///
/// Resolution ladder (no silent read-only fallback after issue #1590):
///   0. If an in-process writer is registered (the OODA daemon does this
///      at startup), return it immediately. Same-process callers skip
///      IPC entirely and share the daemon's writer through `Arc::clone`.
///   1. Try `RemoteCognitiveMemory::connect(default_socket_path())` — if
///      the OODA daemon is running on a different process and owns the
///      same `state_root`, share its writer.
///   2. Otherwise reap any stale open-lock and `NativeCognitiveMemory::open`
///      the DB directly (we own it because no daemon is running).
///
/// Failures at every tier propagate as `SimardError`. The previous
/// "tier 3 = silent open_read_only fallback" path is removed: surfacing
/// the failure loudly is the explicit contract of the launcher (issue
/// #1590, dashboard hollow-success bug).
pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge> {
    let _ = std::fs::create_dir_all(state_root);

    // (0) In-process Arc shortcut — the daemon registers itself at
    // startup via `register_in_process_writer`.
    if let Some(writer) = current_in_process_writer() {
        return Ok(WriterBridge::checked_new(Box::new(SharedMemory(writer))));
    }

    // (1) Prefer the running daemon's IPC writer — but only when our
    // requested state_root actually matches the daemon's. Otherwise we'd
    // route writes to the wrong DB.
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
                "cognitive memory writer failed to open at {}: {e} \
                 (no in-process writer registered, no daemon socket usable; \
                  refusing silent read-only fallback per issue #1590)",
                state_root.display()
            ),
        })?;
    Ok(WriterBridge::checked_new(Box::new(mem)))
}

/// Open a cognitive-memory reader bridge against `state_root`.
///
/// Resolution ladder:
///   0. If an in-process writer is registered (the OODA daemon does this
///      at startup), share it as a reader so we observe live state
///      without contending on disk.
///   1. Try `RemoteCognitiveMemory::connect(default_socket_path())`.
///   2. Otherwise `NativeCognitiveMemory::open_read_only` — fails when the
///      DB has never been opened by a writer.
pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge> {
    // (0) In-process Arc shortcut — sees live writes from the daemon.
    if let Some(writer) = current_in_process_writer() {
        return Ok(ReaderBridge {
            inner: Box::new(SharedMemory(writer)),
        });
    }

    // (1) Prefer the daemon socket when present — but only when the
    // requested state_root matches the daemon's. Otherwise a daemon owning
    // a different DB would mask the read we actually want.
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
