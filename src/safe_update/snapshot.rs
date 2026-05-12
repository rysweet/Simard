//! Phase 2: snapshot the current binary so phase 6 can roll back.
//!
//! Records `current_exe()` path, sha256, mtime and embedded version into
//! `state_dir/last-binary.json`, then copies the live binary to
//! `~/.simard/bin/simard.bak.<utc-iso8601>`. Old backups beyond
//! [`super::state::DEFAULT_BACKUP_RETENTION`] are pruned (oldest first).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::errors::SafeUpdateError;
use super::state::{DEFAULT_BACKUP_RETENTION, now_iso8601};

/// Snapshot of the binary at the moment the orchestration started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySnapshot {
    /// Path to the live binary at snapshot time (typically `current_exe()`).
    pub binary_path: PathBuf,
    /// Hex-encoded sha256 of the binary contents.
    pub sha256: String,
    /// File mtime as UTC ISO-8601 (best effort; `"unknown"` on platforms
    /// without mtime).
    pub mtime: String,
    /// Embedded version (CARGO_PKG_VERSION).
    pub version: String,
    /// Where the rollback copy lives (`~/.simard/bin/simard.bak.<utc>`).
    pub backup_path: PathBuf,
    /// When this snapshot was created (UTC ISO-8601).
    pub captured_at: String,
}

/// Take the current-binary snapshot, write `last-binary.json`, copy the
/// live binary to a timestamped backup and prune old backups.
pub fn take_snapshot(state_dir: &Path) -> Result<BinarySnapshot, SafeUpdateError> {
    let bin = std::env::current_exe().map_err(|e| SafeUpdateError::SnapshotIo {
        action: "current_exe".into(),
        path: PathBuf::from("(current_exe)"),
        reason: e.to_string(),
    })?;
    take_snapshot_of(&bin, state_dir, DEFAULT_BACKUP_RETENTION, default_bin_dir())
}

/// Test-friendly variant: lets the caller substitute the binary path,
/// the retention cap and the backup directory.
pub fn take_snapshot_of(
    binary: &Path,
    state_dir: &Path,
    retention: usize,
    bin_dir: PathBuf,
) -> Result<BinarySnapshot, SafeUpdateError> {
    let bytes = fs::read(binary).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "read".into(),
        path: binary.to_path_buf(),
        reason: e.to_string(),
    })?;
    let sha256 = sha256_hex(&bytes);
    let mtime = mtime_iso8601(binary);
    let version = read_embedded_version(binary, &bytes);

    fs::create_dir_all(&bin_dir).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "mkdir bin_dir".into(),
        path: bin_dir.clone(),
        reason: e.to_string(),
    })?;
    let backup_path = bin_dir.join(format!("simard.bak.{}", now_iso8601_path_safe()));
    fs::write(&backup_path, &bytes).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "write backup".into(),
        path: backup_path.clone(),
        reason: e.to_string(),
    })?;
    set_executable(&backup_path)?;

    let snapshot = BinarySnapshot {
        binary_path: binary.to_path_buf(),
        sha256,
        mtime,
        version,
        backup_path: backup_path.clone(),
        captured_at: now_iso8601(),
    };

    fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "mkdir state_dir".into(),
        path: state_dir.to_path_buf(),
        reason: e.to_string(),
    })?;
    let manifest_path = state_dir.join("last-binary.json");
    let body = serde_json::to_vec_pretty(&snapshot).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "serialize".into(),
        path: manifest_path.clone(),
        reason: e.to_string(),
    })?;
    fs::write(&manifest_path, &body).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "write manifest".into(),
        path: manifest_path,
        reason: e.to_string(),
    })?;

    prune_backups(&bin_dir, retention)?;
    Ok(snapshot)
}

/// Prune old `simard.bak.*` files to at most `retention` entries (newest kept).
///
/// Ordering is based on the timestamped filename (`simard.bak.<utc-iso8601>`)
/// rather than mtime so the ordering matches the human-readable name and
/// is unaffected by filesystem mtime quirks.
pub fn prune_backups(bin_dir: &Path, retention: usize) -> Result<(), SafeUpdateError> {
    let mut backups = list_backups(bin_dir)?;
    if backups.len() <= retention {
        return Ok(());
    }
    // Sort newest-first by filename (timestamp embedded in name).
    backups.sort_by(|a, b| b.cmp(a));
    for path in backups.into_iter().skip(retention) {
        fs::remove_file(&path).map_err(|e| SafeUpdateError::SnapshotIo {
            action: "prune".into(),
            path,
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

/// Locate the newest `simard.bak.*` file in `bin_dir`. Ordering is by
/// filename (timestamped), matching [`prune_backups`].
pub fn latest_backup(bin_dir: &Path) -> Option<PathBuf> {
    let mut backups = list_backups(bin_dir).ok()?;
    backups.sort_by(|a, b| b.cmp(a));
    backups.into_iter().next()
}

/// Read `state_dir/last-binary.json`. Returns `Ok(None)` if absent.
pub fn read_snapshot(state_dir: &Path) -> Result<Option<BinarySnapshot>, SafeUpdateError> {
    let path = state_dir.join("last-binary.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "read manifest".into(),
        path: path.clone(),
        reason: e.to_string(),
    })?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| SafeUpdateError::SnapshotIo {
            action: "parse manifest".into(),
            path,
            reason: e.to_string(),
        })
}

/// Default install/backup directory: `~/.simard/bin/`.
pub fn default_bin_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".simard").join("bin")
    } else {
        PathBuf::from(".simard").join("bin")
    }
}

fn list_backups(bin_dir: &Path) -> Result<Vec<PathBuf>, SafeUpdateError> {
    let mut out: Vec<PathBuf> = Vec::new();
    let entries = match fs::read_dir(bin_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(SafeUpdateError::SnapshotIo {
                action: "read bin_dir".into(),
                path: bin_dir.to_path_buf(),
                reason: e.to_string(),
            });
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("simard.bak.") {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn mtime_iso8601(path: &Path) -> String {
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        }
        Err(_) => "unknown".into(),
    }
}

/// Best-effort version extraction: prefer the compile-time CARGO_PKG_VERSION
/// because it is what the *current* binary embeds. For an arbitrary binary
/// we fall back to scanning the file for an embedded version string of the
/// shape used by `simard --version`. This is intentionally cheap; the
/// snapshot is informational, not load-bearing.
fn read_embedded_version(_path: &Path, bytes: &[u8]) -> String {
    // First: scan for "simard <semver>" — that's how `--version` formats.
    let needle = b"simard ";
    if let Some(pos) = bytes.windows(needle.len()).position(|w| w == needle) {
        let tail = &bytes[pos + needle.len()..];
        let end = tail.iter().position(|&b| !is_version_char(b)).unwrap_or(0);
        if end >= 5 {
            return String::from_utf8_lossy(&tail[..end]).into_owned();
        }
    }
    // Second: fall back to the compiled-in version. This is correct for
    // the *current* binary, which is the common case.
    env!("CARGO_PKG_VERSION").to_string()
}

fn is_version_char(b: u8) -> bool {
    b.is_ascii_digit() || b == b'.' || b == b'-' || b.is_ascii_alphabetic()
}

fn now_iso8601_path_safe() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), SafeUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| SafeUpdateError::SnapshotIo {
            action: "stat for chmod".into(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "chmod 0755".into(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), SafeUpdateError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_fake_binary(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn snapshot_writes_manifest_and_backup() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let bin = make_fake_binary(src.path(), "simard", b"simard 9.9.9\n\x00\x00fake");

        let snap = take_snapshot_of(&bin, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();
        assert_eq!(snap.sha256.len(), 64);
        assert!(state.path().join("last-binary.json").exists());
        assert!(snap.backup_path.starts_with(bin_dir.path()));
        assert!(snap.backup_path.exists());
        // Version extracted from the embedded "simard <semver>" string.
        assert_eq!(snap.version, "9.9.9");
    }

    #[test]
    fn snapshot_falls_back_to_pkg_version_when_string_missing() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let bin = make_fake_binary(
            src.path(),
            "simard",
            b"\x7fELF\x00mock-binary-no-version-string",
        );
        let snap = take_snapshot_of(&bin, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();
        assert_eq!(snap.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn prune_keeps_newest_n_by_filename() {
        let bin_dir = tempdir().unwrap();
        // Create 7 backup files with monotonically increasing timestamp names.
        for i in 0..7 {
            let p = bin_dir
                .path()
                .join(format!("simard.bak.2025-01-0{}T00-00-00Z", i + 1));
            fs::write(&p, b"x").unwrap();
        }
        prune_backups(bin_dir.path(), 3).unwrap();
        let mut kept: Vec<_> = fs::read_dir(bin_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        kept.sort();
        assert_eq!(kept.len(), 3, "kept: {kept:?}");
        // The three NEWEST (by filename) are kept: days 5, 6, 7.
        assert_eq!(
            kept,
            vec![
                "simard.bak.2025-01-05T00-00-00Z".to_string(),
                "simard.bak.2025-01-06T00-00-00Z".to_string(),
                "simard.bak.2025-01-07T00-00-00Z".to_string(),
            ]
        );
    }

    #[test]
    fn latest_backup_returns_newest_by_filename() {
        let bin_dir = tempdir().unwrap();
        let a = bin_dir.path().join("simard.bak.2025-01-01T00-00-00Z");
        let b = bin_dir.path().join("simard.bak.2025-02-01T00-00-00Z");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let latest = latest_backup(bin_dir.path()).unwrap();
        assert_eq!(latest, b);
    }

    #[test]
    fn list_backups_missing_dir_is_empty() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        let v = list_backups(&missing).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn read_snapshot_round_trips() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let bin = make_fake_binary(src.path(), "simard", b"simard 1.2.3 fake-payload");
        let written =
            take_snapshot_of(&bin, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();
        let loaded = read_snapshot(state.path()).unwrap().unwrap();
        assert_eq!(loaded.sha256, written.sha256);
        assert_eq!(loaded.version, "1.2.3");
    }
}
