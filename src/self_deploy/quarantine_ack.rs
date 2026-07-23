//! Durable quarantine acknowledgement (`.ack` sidecars) — issue #4469.
//!
//! When LadybugDB quarantines a corrupt cognitive-memory store it leaves a
//! `cognitive*.corrupt-<ts>` artifact under the state root. The self-health
//! `no_quarantine` probe fails while any such artifact is present, and the
//! #2550 retention rule protects the largest substantial quarantine from the
//! cleanup sweep — so a genuinely-stuck quarantine can freeze self-deploy
//! forever (the probe never clears, but the recovery asset must not be
//! deleted).
//!
//! This module owns the single convention that breaks that deadlock **without
//! destroying data**: a durable, per-artifact `.ack` sidecar. Acknowledging a
//! quarantine writes `<state_root>/<name>.ack`; the probe and the cleanup sweep
//! both treat an artifact with a live `.ack` sidecar as "seen" and stop failing
//! on it, while the quarantined store itself is retained on disk for recovery.
//!
//! The marker is **filename-keyed** (the quarantine name embeds a timestamp),
//! so acknowledging `cognitive.corrupt-20260101` never silences a *new*
//! `cognitive.corrupt-20260202` — fresh corruption still re-fails the probe.
//!
//! ## Contract
//!
//! * [`ack_marker_path`] — the sidecar path for a *valid* corrupt-quarantine
//!   basename directly under `state_root`; `None` for any unsafe / non-quarantine
//!   name (path separators, `..`, absolute paths, the live store).
//! * [`acknowledge`] — idempotently write the sidecar. Never deletes the
//!   quarantined artifact. Refuses unsafe names and refuses to overwrite a
//!   non-regular-file sidecar target (planted symlink defence).
//! * [`is_acknowledged`] — true iff a durable regular-file sidecar exists.
//! * [`is_ack_marker_name`] — true for the sidecar files themselves (`*.ack`),
//!   so scanners never mistake a marker for a quarantine.
//!
//! See `docs/reference/self-deploy-quarantine-acknowledge.md` and
//! `docs/howto/clear-a-stuck-memory-quarantine.md`.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// Suffix appended to a quarantine artifact's name to form its durable
/// acknowledgement sidecar.
pub const ACK_SUFFIX: &str = ".ack";

/// The sidecar's payload. The marker is a presence flag, not a data store;
/// keeping it tiny bounds disk use and forgery blast radius.
const ACK_MARKER_BYTES: &[u8] = b"acknowledged\n";

/// True when `name` is an acknowledgement sidecar (`*.ack`) rather than a
/// quarantine artifact. Scanners MUST exclude these so a marker is never
/// itself treated as a corrupt store.
pub fn is_ack_marker_name(name: &str) -> bool {
    name.ends_with(ACK_SUFFIX)
}

/// True iff `name` is a safe, single-component corrupt-quarantine basename that
/// may be acknowledged: no separators, no `..`/absolute components, non-empty,
/// not itself an `.ack` marker, and a genuine `cognitive*.corrupt-*` artifact.
fn is_ackable_quarantine_basename(name: &str) -> bool {
    if name.is_empty() || is_ack_marker_name(name) {
        return false;
    }
    if name.contains('/') || name.contains('\\') {
        return false;
    }
    // Exactly one Normal component, equal to the whole name (rejects `..`, `.`,
    // absolute prefixes, and anything platform-specific like a drive/root).
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(c)), None) if c == OsStr::new(name) => {}
        _ => return false,
    }
    // Delegate to the canonical predicate so the cleanup sweep, the health
    // probe, and this acknowledge path can never disagree about which artifacts
    // are corrupt-quarantines.
    crate::cmd_cleanup::is_corrupt_quarantine_name(name)
}

/// Compute the durable ack-marker path for the corrupt-quarantine artifact
/// `quarantine_name` directly under `state_root`.
///
/// Returns `None` when `quarantine_name` is not a safe, single-component
/// corrupt-quarantine basename: anything containing a path separator, a `..`
/// component, an absolute path, an empty string, an existing `.ack` marker
/// name, or a name that is not a corrupt-quarantine artifact is rejected.
pub fn ack_marker_path(state_root: &Path, quarantine_name: &str) -> Option<PathBuf> {
    if !is_ackable_quarantine_basename(quarantine_name) {
        return None;
    }
    Some(state_root.join(format!("{quarantine_name}{ACK_SUFFIX}")))
}

/// Build the `PersistentStoreIo` error used for every acknowledgement failure.
fn ack_error(path: PathBuf, reason: impl Into<String>) -> SimardError {
    SimardError::PersistentStoreIo {
        store: "cognitive_memory_quarantine".to_string(),
        action: "acknowledge".to_string(),
        path,
        reason: reason.into(),
    }
}

/// Durably acknowledge the quarantine artifact `quarantine_name` under
/// `state_root` by writing its `.ack` sidecar.
///
/// * **Idempotent** — acknowledging an already-acknowledged artifact succeeds
///   and leaves a single sidecar.
/// * **Non-destructive** — the quarantined artifact itself is never touched.
/// * **Safe** — rejects unsafe names (see [`ack_marker_path`]) and refuses to
///   overwrite a sidecar path that already exists as a non-regular file (a
///   planted symlink or directory), returning `Err` rather than following it.
///
/// Returns the written sidecar path on success.
pub fn acknowledge(state_root: &Path, quarantine_name: &str) -> SimardResult<PathBuf> {
    let marker = ack_marker_path(state_root, quarantine_name).ok_or_else(|| {
        ack_error(
            state_root.join(quarantine_name),
            format!("refusing to acknowledge unsafe or non-quarantine name {quarantine_name:?}"),
        )
    })?;

    // Inspect the sidecar path WITHOUT following symlinks. A pre-existing
    // regular file means the artifact is already acknowledged (idempotent);
    // anything else at that path (symlink, directory) is a hostile plant we
    // refuse to touch rather than write through.
    match std::fs::symlink_metadata(&marker) {
        Ok(meta) if meta.file_type().is_file() => return Ok(marker),
        Ok(_) => {
            return Err(ack_error(
                marker,
                "sidecar path already exists as a non-regular file (symlink/dir); refusing to overwrite",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ack_error(marker, format!("stat sidecar: {e}"))),
    }

    // `create_new` opens with O_EXCL: it never follows a symlink and fails if
    // the path already exists, closing the TOCTOU window from the stat above.
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(f) => f,
        // Lost a race but the winner left a regular file — still acknowledged.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return match std::fs::symlink_metadata(&marker) {
                Ok(meta) if meta.file_type().is_file() => Ok(marker),
                _ => Err(ack_error(
                    marker,
                    "sidecar path raced into a non-regular file; refusing to overwrite",
                )),
            };
        }
        Err(e) => return Err(ack_error(marker, format!("create sidecar: {e}"))),
    };
    file.write_all(ACK_MARKER_BYTES)
        .and_then(|()| file.sync_all())
        .map_err(|e| ack_error(marker.clone(), format!("write sidecar: {e}")))?;
    Ok(marker)
}

/// True when a durable regular-file `.ack` sidecar exists for
/// `quarantine_name` under `state_root`. A non-regular-file at the sidecar
/// path (symlink, directory) is NOT a valid acknowledgement.
pub fn is_acknowledged(state_root: &Path, quarantine_name: &str) -> bool {
    match ack_marker_path(state_root, quarantine_name) {
        Some(marker) => std::fs::symlink_metadata(&marker)
            .map(|meta| meta.file_type().is_file())
            .unwrap_or(false),
        None => false,
    }
}

/// List the acknowledgeable corrupt-quarantine artifact basenames present
/// directly under `state_root` (issue #4469).
///
/// Excludes `.ack` sidecars and anything that is not a safe, single-component
/// `cognitive*.corrupt-*` artifact (so the live store is never returned). The
/// operator `--acknowledge-quarantine` path iterates this list, keeping
/// `quarantine_ack` the single owner of "what is an acknowledgeable quarantine".
/// Absent/unreadable dir ⇒ empty.
pub fn present_quarantine_artifacts(state_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(state_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_ackable_quarantine_basename(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUARANTINE: &str = "cognitive.corrupt-20260101120000";

    // ── is_ack_marker_name ──

    #[test]
    fn ack_marker_name_matches_only_ack_suffix() {
        assert!(is_ack_marker_name("cognitive.corrupt-20260101.ack"));
        assert!(!is_ack_marker_name("cognitive.corrupt-20260101"));
        assert!(!is_ack_marker_name("cognitive"));
    }

    // ── ack_marker_path ──

    #[test]
    fn marker_path_is_sibling_with_ack_suffix() {
        let root = Path::new("/var/lib/simard");
        let marker = ack_marker_path(root, QUARANTINE).expect("valid quarantine name");
        assert_eq!(marker, root.join(format!("{QUARANTINE}{ACK_SUFFIX}")));
        // Marker is a single component directly under the state root.
        assert_eq!(marker.parent(), Some(root));
        assert_eq!(
            marker.file_name().and_then(|s| s.to_str()),
            Some(format!("{QUARANTINE}{ACK_SUFFIX}").as_str())
        );
    }

    #[test]
    fn marker_path_rejects_path_separators() {
        let root = Path::new("/var/lib/simard");
        assert!(ack_marker_path(root, "cognitive.corrupt-1/evil").is_none());
        assert!(ack_marker_path(root, "sub/cognitive.corrupt-1").is_none());
        assert!(ack_marker_path(root, "cognitive.corrupt-1\\evil").is_none());
    }

    #[test]
    fn marker_path_rejects_parent_and_absolute() {
        let root = Path::new("/var/lib/simard");
        assert!(ack_marker_path(root, "..").is_none());
        assert!(ack_marker_path(root, "../cognitive.corrupt-1").is_none());
        assert!(ack_marker_path(root, "/etc/passwd").is_none());
        assert!(ack_marker_path(root, "").is_none());
    }

    #[test]
    fn marker_path_rejects_non_quarantine_and_marker_names() {
        let root = Path::new("/var/lib/simard");
        // The live store and unrelated files are not acknowledgeable.
        assert!(ack_marker_path(root, "cognitive").is_none());
        assert!(ack_marker_path(root, "cognitive.wal").is_none());
        assert!(ack_marker_path(root, "unrelated.corrupt-1").is_none());
        // An existing marker must not be re-acknowledged into `*.ack.ack`.
        assert!(ack_marker_path(root, "cognitive.corrupt-1.ack").is_none());
    }

    // ── acknowledge / is_acknowledged ──

    #[test]
    fn acknowledge_creates_durable_marker_and_retains_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join(QUARANTINE);
        std::fs::write(&artifact, b"quarantined-store-bytes").unwrap();

        assert!(!is_acknowledged(dir.path(), QUARANTINE));

        let marker = acknowledge(dir.path(), QUARANTINE).expect("acknowledge succeeds");
        assert!(marker.is_file(), "sidecar must be a regular file");
        assert!(is_acknowledged(dir.path(), QUARANTINE));
        // Non-destructive: the quarantined artifact is retained for recovery.
        assert!(artifact.is_file(), "quarantine artifact must be retained");
    }

    #[test]
    fn acknowledge_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(QUARANTINE), b"x").unwrap();

        let first = acknowledge(dir.path(), QUARANTINE).unwrap();
        let second = acknowledge(dir.path(), QUARANTINE).unwrap();
        assert_eq!(first, second, "same marker path on repeat ack");
        assert!(is_acknowledged(dir.path(), QUARANTINE));

        // Exactly one sidecar exists for this artifact.
        let markers = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| is_ack_marker_name(&e.file_name().to_string_lossy()))
            .count();
        assert_eq!(markers, 1);
    }

    #[test]
    fn acknowledge_rejects_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        assert!(acknowledge(dir.path(), "../escape").is_err());
        assert!(acknowledge(dir.path(), "sub/cognitive.corrupt-1").is_err());
        assert!(acknowledge(dir.path(), "/etc/passwd").is_err());
        assert!(acknowledge(dir.path(), "cognitive").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn acknowledge_refuses_to_overwrite_planted_symlink_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(QUARANTINE), b"x").unwrap();

        // Plant a hostile sidecar that points at a sensitive file.
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"original").unwrap();
        let marker = dir.path().join(format!("{QUARANTINE}{ACK_SUFFIX}"));
        std::os::unix::fs::symlink(&victim, &marker).unwrap();

        // Acknowledgement must refuse rather than follow the symlink.
        assert!(
            acknowledge(dir.path(), QUARANTINE).is_err(),
            "must not overwrite a non-regular-file sidecar"
        );
        // The victim's contents must be untouched (no write-through).
        assert_eq!(std::fs::read(&victim).unwrap(), b"original");
        // A symlink is not a valid acknowledgement.
        assert!(!is_acknowledged(dir.path(), QUARANTINE));
    }

    #[test]
    fn is_acknowledged_false_without_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(QUARANTINE), b"x").unwrap();
        assert!(!is_acknowledged(dir.path(), QUARANTINE));
    }
}
