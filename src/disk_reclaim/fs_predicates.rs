//! Shared fail-closed filesystem predicates for disk reclamation (issue #4810).
//!
//! Every deletion path in this module must confirm — immediately before acting
//! — that its target is a **real, non-symlink directory owned by the effective
//! UID**. That check previously lived inline in three call sites
//! ([`build_cache::vetted_leaf`](super::build_cache), the guard's
//! [`is_registered_build_cache_leaf`](super::guard), and the executor's
//! pre-unlink re-stat); the predicate is centralized here so the symlink-swap
//! and ownership-swap defenses cannot drift apart.
//!
//! Each caller obtains its own metadata via [`std::fs::symlink_metadata`] (so
//! the final path component is **never** followed) and passes the result here
//! for the type/ownership verdict.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;

/// `true` iff `meta` describes a real (non-symlink) directory.
///
/// The caller must have obtained `meta` via [`std::fs::symlink_metadata`], so a
/// symlink is reported as a symlink and rejected rather than being resolved to
/// its (possibly foreign or protected) target.
pub(super) fn is_real_dir(meta: &Metadata) -> bool {
    !meta.file_type().is_symlink() && meta.is_dir()
}

/// `true` iff `meta` describes a real, non-symlink directory owned by the
/// effective UID.
///
/// This is the shared pre-deletion predicate: [`is_real_dir`] plus a same-owner
/// check that confines every reclaim action to directories the current process
/// already owns, closing symlink-swap and ownership-swap TOCTOU windows.
pub(super) fn is_real_dir_owned_by_euid(meta: &Metadata) -> bool {
    if !is_real_dir(meta) {
        return false;
    }
    // SAFETY: `geteuid` takes no arguments, reads no memory, and cannot fail.
    let euid = unsafe { libc::geteuid() };
    meta.uid() == euid
}
