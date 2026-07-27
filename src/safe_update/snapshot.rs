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
    // The self-deploy trigger derives `binary` from `current_exe()`. On Linux a
    // still-running image whose on-disk file was unlinked by a prior swap resolves
    // to `<path> (deleted)`, which no longer exists; reading it directly fails with
    // `No such file` and aborted every deploy at the mandatory protective backup
    // (DeployDrift, issue #4857 / #4836). The LIVE running image stays readable via
    // `/proc/self/exe` even after its directory entry is unlinked, so degrade to it.
    // An existing declared path is read verbatim — the fallback never fires when the
    // path exists, so it can never mask a genuine wrong-path bug.
    let source = resolve_readable_binary(binary);
    let bytes = fs::read(&source).map_err(|e| SafeUpdateError::SnapshotIo {
        action: "read".into(),
        path: source.clone(),
        reason: e.to_string(),
    })?;
    let sha256 = sha256_hex(&bytes);
    let mtime = mtime_iso8601(&source);
    let version = read_embedded_version(&source, &bytes);

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
        binary_path: source.clone(),
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

/// Resolve the path actually read for the binary snapshot, degrading to the LIVE
/// running image when the declared on-disk path is missing.
///
/// The self-deploy trigger derives the declared path from `current_exe()`. On
/// Linux, once a prior self-deploy has swapped the on-disk binary, a still-running
/// old image's `current_exe()` resolves to `<path> (deleted)` and that file no
/// longer exists, so a direct read fails with `No such file`. Because the
/// protective backup is mandatory, that hard-failure aborted EVERY deploy and
/// stranded the running binary behind merged main (DeployDrift, issue #4857 /
/// #4836).
///
/// `/proc/self/exe` still yields the bytes of the running image even after its
/// directory entry is unlinked (the inode is held open by the running process),
/// so when the declared path is missing we snapshot through it. When the declared
/// path exists it is returned unchanged, so the fallback can never fire for a
/// present-but-wrong path and thus cannot mask an unrelated bug. On platforms
/// without `/proc/self/exe` (e.g. macOS) the declared path is returned so the
/// existing loud failure is preserved.
fn resolve_readable_binary(declared: &Path) -> PathBuf {
    if declared.exists() {
        return declared.to_path_buf();
    }
    #[cfg(target_os = "linux")]
    {
        let running_image = Path::new("/proc/self/exe");
        if running_image.exists() {
            tracing::warn!(
                declared = %declared.display(),
                fallback = "/proc/self/exe",
                "self-deploy snapshot: declared binary path is missing (unlinked \
                 running image); snapshotting the LIVE running image so the \
                 mandatory protective backup does not deadlock the deploy (issue #4857)"
            );
            return running_image.to_path_buf();
        }
    }
    declared.to_path_buf()
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

/// TDD (Step 7) — FAILING tests pinning the "back up the RUNNING image, not the
/// deleted on-disk path" contract for the issue #4857 self-deploy deadlock.
///
/// # The defect these tests lock down
///
/// The protective binary backup snapshots `install_path`, which the deploy
/// trigger derives from `std::env::current_exe()`. On Linux, once a prior
/// self-deploy has replaced the on-disk binary, a *still-running* old image's
/// `current_exe()` resolves to `<path> (deleted)` and that on-disk file no
/// longer exists. [`take_snapshot_of`] then does `fs::read(binary)` and fails
/// with `SnapshotIo { action: "read", reason: "No such file (os error 2)" }` —
/// the exact `snapshot read on /home/azureuser/.simard/bin/simard (deleted): No
/// such file` failure observed on deploy `56755c8e8454`. Because the backup is
/// mandatory, EVERY deploy aborts, so the running binary drifts ever further
/// behind merged main (DeployDrift) and no self-improvement ever ships.
///
/// # The contract (what the fix must make true)
///
/// A snapshot whose declared on-disk path is missing/deleted MUST degrade to
/// reading the **live running image** rather than hard-failing:
///   * on Linux the running image is always readable via `/proc/self/exe` even
///     after its directory entry is unlinked (the inode is held open by the
///     running process), so the backup must read those bytes;
///   * the resulting binary backup must be written and byte-identical to the
///     running image — a real, restorable rollback artifact, not a stub;
///   * the happy path is unchanged: when the declared path DOES exist, its bytes
///     are snapshotted verbatim and the running-image fallback is NOT used (so
///     the fallback can never mask an unrelated wrong-path bug).
///
/// These tests compile against the CURRENT public symbols and fail on
/// BEHAVIOUR: today a deleted path returns `Err(SnapshotIo { action: "read" })`.
#[cfg(all(test, target_os = "linux"))]
mod tests_deleted_running_image {
    use super::*;
    use tempfile::tempdir;

    /// Bytes of the live running image (this test binary). After a swap the
    /// inode behind `/proc/self/exe` stays readable even though the on-disk name
    /// is `(deleted)`, so this is exactly what the protective backup must
    /// capture when the declared install path is gone.
    fn running_image_bytes() -> Vec<u8> {
        fs::read("/proc/self/exe").expect("the running image must be readable via /proc/self/exe")
    }

    /// T1 — the core deadlock-breaker. A `<path> (deleted)` install path (the
    /// literal string `current_exe()` hands back post-swap) whose on-disk file
    /// is absent must NOT abort the snapshot; it must degrade to the running
    /// image so the deploy can still ship. Today: `Err(SnapshotIo read)`.
    #[test]
    fn deleted_install_path_degrades_to_running_image() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        // Mirror the post-swap install path: same "(deleted)" suffix, no file.
        let deleted = bin_dir.path().join("simard (deleted)");
        assert!(!deleted.exists(), "precondition: the on-disk file is gone");

        let snap = take_snapshot_of(
            &deleted,
            state.path(),
            DEFAULT_BACKUP_RETENTION,
            bin_dir.path().to_path_buf(),
        )
        .expect(
            "a deleted on-disk install path must degrade to the running image, \
             not hard-fail the mandatory protective backup (issue #4857)",
        );

        assert!(
            snap.backup_path.exists(),
            "a binary backup of the running image must be written"
        );
        let backed_up = fs::read(&snap.backup_path).unwrap();
        assert!(
            !backed_up.is_empty(),
            "the running-image backup must be non-empty"
        );
        assert_eq!(
            backed_up,
            running_image_bytes(),
            "the backup must be byte-identical to the LIVE running image so it is \
             a restorable rollback artifact, not a stub",
        );
    }

    /// T2 — a non-suffixed but simply-absent install path (e.g. the on-disk file
    /// was unlinked without the `(deleted)` annotation) is the same degrade: the
    /// running image is snapshotted rather than the backup aborting.
    #[test]
    fn absent_install_path_degrades_to_running_image() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let absent = bin_dir.path().join("simard");
        assert!(!absent.exists());

        let snap = take_snapshot_of(
            &absent,
            state.path(),
            DEFAULT_BACKUP_RETENTION,
            bin_dir.path().to_path_buf(),
        )
        .expect("an absent running-image path must degrade to the live image, not abort");
        assert_eq!(
            fs::read(&snap.backup_path).unwrap(),
            running_image_bytes(),
            "an absent declared path must fall back to the running image bytes",
        );
    }

    /// T3 — regression guard: the fallback must ONLY trigger when the declared
    /// path is unreadable. An EXISTING path is still snapshotted verbatim, so
    /// the degrade can never mask a genuine wrong-path bug by silently backing
    /// up the running image instead of the file the caller named.
    #[test]
    fn existing_install_path_is_read_verbatim_not_the_running_image() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let declared = src.path().join("simard");
        // Distinct sentinel content that is NOT the running image.
        fs::write(&declared, b"simard 4.8.5 declared-on-disk-payload").unwrap();

        let snap = take_snapshot_of(
            &declared,
            state.path(),
            DEFAULT_BACKUP_RETENTION,
            bin_dir.path().to_path_buf(),
        )
        .unwrap();
        let backed_up = fs::read(&snap.backup_path).unwrap();
        assert_eq!(
            backed_up, b"simard 4.8.5 declared-on-disk-payload",
            "an existing declared path must be snapshotted verbatim",
        );
        assert_ne!(
            backed_up,
            running_image_bytes(),
            "the running-image fallback must NOT fire when the declared path exists",
        );
    }
}
