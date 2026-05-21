//! Shared fsync helpers for the cognitive-memory durability barrier.
//!
//! Three call sites in this crate need to issue the same `open(2)` +
//! `fsync(2)` pair against a path and map every IO failure into a typed
//! [`SimardError::PersistentStoreIo`]:
//!
//! 1. [`super::NativeCognitiveMemory::post_write_barrier`] — fsyncs the
//!    data file and its parent directory after every mutating op
//!    (issue #1973, decision D2).
//! 2. [`super::backup::NativeCognitiveMemory::fsync_recovery_replay`]
//!    (private) — fsyncs a freshly-restored DB and its parent directory
//!    after `try_restore_from_backup` copies in a candidate backup
//!    (issue #1973, decision D4).
//! 3. [`super::backup`]'s `atomic_copy_with_fsync` — fsyncs the staged
//!    `.tmp` backup file and the destination's parent directory before
//!    declaring the backup durable (issue #1973, decision D3).
//!
//! Each site had its own ~12-line `OpenOptions/sync_all/map_err` pair
//! per fsync step before this module existed; consolidating them here
//! removes ~50 lines of structurally-identical boilerplate while
//! preserving the per-site action labels that #1975 and operator logs
//! grep on.

use std::path::Path;

use crate::error::{SimardError, SimardResult};

/// Open `path` read-only and `sync_all` it, attributing every IO
/// failure to the cognitive-memory store with the supplied action
/// labels.
///
/// `open_action` and `sync_action` are kept distinct because operators
/// and the #1975 silent-IO-failure audit grep on these strings — a
/// failure to open the path for fsync is a different operational
/// signal than a failure of the fsync syscall itself.
///
/// `reason_context` is woven into the error `reason` string when
/// non-empty (use `""` when no per-call context applies). Typical
/// usage: `"op=store_fact"` from the per-write barrier, so a fsync
/// failure can be attributed back to the mutating op that triggered it.
///
/// Works for both regular files and directories — `File::open` on a
/// directory is the canonical Unix way to obtain a fd suitable for
/// `fsync(2)` on the dirent.
pub(super) fn open_and_fsync(
    path: &Path,
    open_action: &str,
    sync_action: &str,
    reason_context: &str,
) -> SimardResult<()> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| io_err(open_action, path, reason_context, e))?;
    f.sync_all()
        .map_err(|e| io_err(sync_action, path, reason_context, e))?;
    Ok(())
}

fn io_err(action: &str, path: &Path, ctx: &str, e: std::io::Error) -> SimardError {
    let reason = if ctx.is_empty() {
        e.to_string()
    } else {
        format!("{ctx}: {e}")
    };
    SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: action.into(),
        path: path.to_path_buf(),
        reason,
    }
}
