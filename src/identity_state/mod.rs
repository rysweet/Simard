//! Generic **identity-scoped curated data** — the framework mechanism that lets
//! each identity carry its own differently-typed, agentically-curated, mutable
//! state (repos for Simard, menus for a Gastronome, …) WITHOUT the framework
//! hardcoding what that data *is*.
//!
//! The store is deliberately generic: it persists an arbitrary named TOML
//! document, addressed by `(identity, key)`, under the durable Simard state
//! root:
//!
//! ```text
//! <state_root>/identity/<identity>/curated/<key>.toml
//! ```
//!
//! Two properties make this the right home for identity-owned state:
//!
//! 1. **Deploy-durable.** `install` (and self-deploy) only ever rewrites the
//!    binary, the systemd units, and `~/.simard/prompt_assets/`. It NEVER writes
//!    under `<state_root>/identity/`, so an identity's runtime edits survive
//!    every re-deploy — unlike the old git-tracked `prompt_assets` roster, which
//!    each self-deploy clobbered from the repo (see the `prompt_assets.*.bak`
//!    install backups).
//! 2. **Identity-scoped.** Keyed by identity, so two identities on one host keep
//!    independent curated data (consistent with
//!    [`crate::state_root`] per-identity `SIMARD_STATE_ROOT` isolation).
//!
//! The module knows nothing about "repos" or "rosters" — a typed view (e.g. the
//! stewarded-roster view in [`crate::overseer::ecosystem_observe`]) is layered on
//! top by parsing the raw TOML it returns. Writes are atomic (temp file +
//! `rename`) so a concurrent reader never observes a torn document.

use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// The per-state-root subdirectory that roots all identity-scoped curated data.
pub const IDENTITY_SUBDIR: &str = "identity";
/// The per-identity subdirectory that holds curated documents.
pub const CURATED_SUBDIR: &str = "curated";

/// Whether `component` is a safe single path segment for use as an `identity` or
/// `key`: non-empty, no path traversal, no separators, only `[A-Za-z0-9._-]`.
/// This is the same containment the roster slug validator applies, so a
/// malformed identity/key can never escape `<state_root>/identity/`.
fn is_safe_component(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    if component.contains("..") {
        return false;
    }
    component
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Trim and validate `raw` as a safe path component, returning the owned segment
/// or `None` if it is unsafe. Callers use this to sanitize an untrusted identity
/// name (e.g. from `SIMARD_IDENTITY`) before it addresses a curated file.
pub fn sanitize_component(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if is_safe_component(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Resolve the curated-data file path for `(identity, key)` under `state_root`.
///
/// Returns `None` when either `identity` or `key` is not a safe path component
/// (path-traversal prevention) — the caller then treats the data as absent /
/// errors, never touching an out-of-tree path.
pub fn curated_data_path(state_root: &Path, identity: &str, key: &str) -> Option<PathBuf> {
    let identity = sanitize_component(identity)?;
    let key = sanitize_component(key)?;
    Some(
        state_root
            .join(IDENTITY_SUBDIR)
            .join(identity)
            .join(CURATED_SUBDIR)
            .join(format!("{key}.toml")),
    )
}

/// Load the raw TOML text of the curated document `(identity, key)`.
///
/// Returns `Some(text)` when the document exists and is readable, `None` when it
/// is absent, the path is unresolvable (unsafe component), or the read fails.
/// The absence case is the signal callers use to seed the document on first use.
pub fn load_curated(state_root: &Path, identity: &str, key: &str) -> Option<String> {
    let path = curated_data_path(state_root, identity, key)?;
    std::fs::read_to_string(&path).ok()
}

/// Whether the curated document `(identity, key)` already exists on disk.
pub fn curated_exists(state_root: &Path, identity: &str, key: &str) -> bool {
    curated_data_path(state_root, identity, key).is_some_and(|p| p.is_file())
}

/// Atomically write `contents` as the curated document `(identity, key)`,
/// creating the parent directories if needed.
///
/// The write is a temp file + `rename` on the same filesystem, so a concurrent
/// reader sees either the old or the new document whole — never a torn write.
pub fn store_curated(
    state_root: &Path,
    identity: &str,
    key: &str,
    contents: &str,
) -> SimardResult<()> {
    let path =
        curated_data_path(state_root, identity, key).ok_or_else(|| SimardError::ArtifactIo {
            path: state_root.join(IDENTITY_SUBDIR),
            reason: format!(
                "invalid identity/key for curated data (identity={identity:?}, key={key:?}): \
                 must be a clean path component"
            ),
        })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SimardError::ArtifactIo {
            path: parent.to_path_buf(),
            reason: format!("creating curated-data dir: {e}"),
        })?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents.as_bytes()).map_err(|e| SimardError::ArtifactIo {
        path: tmp.clone(),
        reason: format!("writing curated-data temp file: {e}"),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("renaming curated-data temp file into place: {e}"),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_scoped_under_identity_curated() {
        let root = Path::new("/state");
        let p = curated_data_path(root, "simard", "stewarded_repos").unwrap();
        assert_eq!(
            p,
            Path::new("/state/identity/simard/curated/stewarded_repos.toml")
        );
    }

    #[test]
    fn unsafe_components_are_rejected() {
        let root = Path::new("/state");
        assert!(curated_data_path(root, "..", "k").is_none());
        assert!(curated_data_path(root, "a/b", "k").is_none());
        assert!(curated_data_path(root, "id", "../escape").is_none());
        assert!(curated_data_path(root, "id", "a/b").is_none());
        assert!(curated_data_path(root, "", "k").is_none());
        assert!(curated_data_path(root, "id", "").is_none());
        assert!(sanitize_component("ok-name.1_2").is_some());
        assert!(sanitize_component("../nope").is_none());
    }

    #[test]
    fn store_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(!curated_exists(root, "simard", "stewarded_repos"));
        assert!(load_curated(root, "simard", "stewarded_repos").is_none());

        store_curated(root, "simard", "stewarded_repos", "schema_version = 1\n").unwrap();

        assert!(curated_exists(root, "simard", "stewarded_repos"));
        assert_eq!(
            load_curated(root, "simard", "stewarded_repos").as_deref(),
            Some("schema_version = 1\n")
        );
    }

    #[test]
    fn store_overwrites_atomically_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        store_curated(root, "simard", "k", "first").unwrap();
        store_curated(root, "simard", "k", "second").unwrap();
        assert_eq!(load_curated(root, "simard", "k").as_deref(), Some("second"));
        let curated_dir = root.join("identity/simard/curated");
        let leftovers: Vec<_> = std::fs::read_dir(&curated_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write must leave no .tmp file");
    }

    #[test]
    fn two_identities_keep_independent_curated_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        store_curated(root, "simard", "k", "simard-data").unwrap();
        store_curated(root, "gastronome", "k", "menu-data").unwrap();
        assert_eq!(
            load_curated(root, "simard", "k").as_deref(),
            Some("simard-data")
        );
        assert_eq!(
            load_curated(root, "gastronome", "k").as_deref(),
            Some("menu-data")
        );
    }

    #[test]
    fn store_rejects_unsafe_component() {
        let dir = tempfile::tempdir().unwrap();
        assert!(store_curated(dir.path(), "..", "k", "x").is_err());
    }
}
