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
use std::sync::Arc;

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::{SimardError, SimardResult};

use super::{RemoteCognitiveMemory, default_socket_path, default_state_root, reap_stale_open_lock};

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

    /// Test-only constructor.
    ///
    /// Wraps a caller-provided `Box<dyn CognitiveMemoryOps>` as a
    /// `WriterBridge` so tests can pin the defensive
    /// "writer must not be read-only" invariant directly.
    ///
    /// Production code MUST go through [`launch_writer_bridge`] so the
    /// resolution ladder runs. Step 8 of issue #1590 follow-up will add
    /// the `assert!(!inner.is_read_only())` invariant to this
    /// constructor and route all production [`WriterBridge`] construction
    /// through it — pinned by
    /// `writer_bridge_construction_panics_when_inner_is_read_only`.
    #[cfg(test)]
    pub fn from_ops_for_test(inner: Box<dyn CognitiveMemoryOps>) -> Self {
        Self { inner }
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
///
/// **TDD stub** — the implementation lands in step 8 of the issue #1590
/// follow-up. Today this is a no-op so [`launch_writer_bridge`] still
/// proceeds through tiers 1–3 unchanged.
pub fn register_in_process_writer(writer: Arc<dyn CognitiveMemoryOps>) {
    let _ = writer;
    // TDD stub — see `IN_PROCESS_WRITER` in step 8.
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
/// Resolution ladder:
///   1. Try `RemoteCognitiveMemory::connect(default_socket_path())` — if the
///      OODA daemon is running, share its writer.
///   2. Otherwise reap any stale open-lock and `NativeCognitiveMemory::open`
///      the DB directly (we own it because no daemon is running).
///   3. Last-resort: `NativeCognitiveMemory::open_read_only` — recoverable
///      degradation; later write calls will surface their own errors.
pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge> {
    let _ = std::fs::create_dir_all(state_root);

    // (1) Prefer the running daemon's IPC writer — but only when our
    // requested state_root actually matches the daemon's. Otherwise we'd
    // route writes to the wrong DB.
    let sock = default_socket_path();
    if sock.exists() && state_root_matches_daemon(state_root) {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                return Ok(WriterBridge {
                    inner: Box::new(client),
                });
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

    match NativeCognitiveMemory::open(state_root) {
        Ok(mem) => Ok(WriterBridge {
            inner: Box::new(mem),
        }),
        Err(rw_err) => {
            // (3) Last-resort read-only fallback.
            eprintln!(
                "[simard] launch_writer_bridge: read-write open failed ({rw_err}); falling back \
                 to read-only — write attempts will surface errors at call time"
            );
            let mem = NativeCognitiveMemory::open_read_only(state_root).map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "memory-ipc-launcher".into(),
                    reason: format!(
                        "cognitive memory failed to open even read-only at {}: {e}",
                        state_root.display()
                    ),
                }
            })?;
            Ok(WriterBridge {
                inner: Box::new(mem),
            })
        }
    }
}

/// Open a cognitive-memory reader bridge against `state_root`.
///
/// Resolution ladder:
///   1. Try `RemoteCognitiveMemory::connect(default_socket_path())`.
///   2. Otherwise `NativeCognitiveMemory::open_read_only` — fails when the
///      DB has never been opened by a writer.
pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge> {
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
