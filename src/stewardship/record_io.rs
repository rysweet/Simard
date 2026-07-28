//! Shared durable-record IO primitives for the typed agentic-recipe transport
//! stores and the thin rails that read them (issue #4911).
//!
//! Both typed stores ([`super::merge_verdict_store`],
//! [`super::liaison_decision_store`]) and the deterministic rails
//! ([`crate::overseer::signal_liaison`], [`crate::overseer::rework_loop`]) plus
//! the liaison WRITE tool ([`crate::operator_cli`]) previously each carried their
//! own copy of the *same* two primitives:
//!
//!   * an atomic, owner-only (`0o600`) temp-write + `rename`, and
//!   * a SHA-256 → path-safe hex segment.
//!
//! Five copies of a subtle correctness-critical write path is exactly the drift
//! hazard (e.g. a missing `0o600` or `fsync`) this module removes: there is now
//! ONE implementation, tested once, reused everywhere.

use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

/// SHA-256 of `data` rendered as a lowercase hex string — a single path-safe
/// segment (no `/`, `+`, `=`, or other separators), so an opaque/base64 input
/// can never appear verbatim in a path or escape a subtree.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Atomically write `bytes` to `path`, owner-only (`0o600`).
///
/// Creates parent directories, writes to a hidden `pid`+`nanos`-uniquified temp
/// sibling, `fsync`s it, sets `0o600` on the temp file (so the atomic `rename`
/// lands an already-restricted file with no permissions window), then `rename`s
/// over the final path (last writer wins). A concurrent reader therefore never
/// observes a partial or world-readable record, and a failed `rename` leaves no
/// temp file behind (best-effort cleanup).
pub fn atomic_write_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("path {path:?} has no parent directory"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all {dir:?} failed: {e}"))?;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".record.tmp.{}.{nanos}", std::process::id()));
    {
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("create temp {tmp:?} failed: {e}"))?;
        f.write_all(bytes)
            .map_err(|e| format!("write temp {tmp:?} failed: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("fsync temp {tmp:?} failed: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("chmod 0o600 temp {tmp:?} failed: {e}"))?;
        }
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {tmp:?} -> {path:?} failed: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_lowercase_hex_of_the_known_vector() {
        // The canonical SHA-256 of the empty input, lowercase hex.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Deterministic and free of any path separator / base64 punctuation.
        let seg = sha256_hex(b"group/id+with=base64/chars");
        assert_eq!(seg.len(), 64);
        assert!(
            seg.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn atomic_write_creates_parents_overwrites_and_leaves_no_temp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("deep").join("rec.json");

        atomic_write_0600(&path, b"first").expect("first write");
        assert_eq!(std::fs::read(&path).unwrap(), b"first");

        // Last writer wins.
        atomic_write_0600(&path, b"second").expect("overwrite");
        assert_eq!(std::fs::read(&path).unwrap(), b"second");

        // No temp files stranded beside the record.
        let strays: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".record.tmp"))
            .collect();
        assert!(strays.is_empty(), "no temp files must remain: {strays:?}");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rec.json");
        atomic_write_0600(&path, b"x").expect("write");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "record must be owner-only");
    }
}
