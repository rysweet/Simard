//! Generic identity-scoped, mutable, deploy-durable typed data store.
//!
//! An identity is separable from the framework like the example identities
//! (Gastronome/Atelier/Maestro …): each carries its own **differently-typed**
//! curated data — a Gastronome carries menus and events, Simard carries the set
//! of repos she stewards. This module is the GENERIC mechanism that lets an
//! identity own such data as *mutable runtime state*, not as a committed
//! framework file.
//!
//! ## Why this exists (deploy-durability)
//!
//! The data lives under the durable **state root**
//! ([`crate::state_root::simard_state_root`] — `$SIMARD_STATE_ROOT` or
//! `$HOME/.simard`), in `identity_state/<identity>/<key>.toml`. A self-deploy
//! (`simard install`) only replaces `~/.simard/prompt_assets`; it never touches
//! the state root. So a runtime edit — an identity curating its own data
//! agentically — SURVIVES a re-deploy instead of being clobbered by the copy
//! baked into the release, which is the whole point: the data is part of *who
//! the identity is*, owned and mutable, not a framework file re-installed from
//! the repo on every deploy.
//!
//! ## What the framework core knows (nothing domain-specific)
//!
//! This store is deliberately **payload-agnostic**: it reads and writes
//! arbitrary [`serde`] values keyed by `(identity, key)`. It has no notion of
//! "roster" or "menu" — consumers define their own typed payloads and seeds.
//! That keeps the "list of stewarded repos" concept OUT of framework code: the
//! framework provides only this generic identity-scoped-mutable-data rail.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{SimardError, SimardResult};

/// Subdirectory, under the durable state root, that holds every identity's
/// curated state. Hardcoded const — never derived from env/args/file contents
/// (path-traversal prevention: only [`is_safe_segment`]-validated identity and
/// key names are joined beneath it).
pub const IDENTITY_STATE_SUBDIR: &str = "identity_state";

/// The identity slug used when no identity is explicitly selected — Simard
/// herself (the default identity).
pub const DEFAULT_IDENTITY_SLUG: &str = "simard";

/// Environment variable that selects the active identity at runtime. Mirrors the
/// daemon's `SIMARD_IDENTITY` resolution so the CLI and the daemon agree on
/// whose curated state to read.
pub const IDENTITY_ENV: &str = "SIMARD_IDENTITY";

/// Resolve the active identity slug from the environment, falling back to
/// [`DEFAULT_IDENTITY_SLUG`] (Simard). A value that is not a clean path segment
/// (empty, `.`/`..`, a separator, or any character outside `[A-Za-z0-9._-]`) is
/// rejected in favour of the default with a logged warning, so a malformed
/// `SIMARD_IDENTITY` can never escape the state-root directory.
pub fn active_identity_slug() -> String {
    match std::env::var(IDENTITY_ENV) {
        Ok(name) => {
            let trimmed = name.trim();
            if is_safe_segment(trimmed) {
                trimmed.to_string()
            } else if trimmed.is_empty() {
                DEFAULT_IDENTITY_SLUG.to_string()
            } else {
                tracing::warn!(
                    target: "identity_state",
                    identity = %trimmed,
                    "SIMARD_IDENTITY is not a clean path segment; falling back to the default identity"
                );
                DEFAULT_IDENTITY_SLUG.to_string()
            }
        }
        Err(_) => DEFAULT_IDENTITY_SLUG.to_string(),
    }
}

/// A path segment is safe iff it is a non-empty string of `[A-Za-z0-9._-]` that
/// is neither `.` nor `..`. This blocks path traversal (`..`), absolute
/// escapes (a leading `/`), and NUL/whitespace injection before an identity or
/// key name is ever joined onto the state-root path.
fn is_safe_segment(segment: &str) -> bool {
    if segment.is_empty() || segment == "." || segment == ".." {
        return false;
    }
    segment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// A generic per-identity, per-key mutable store rooted at
/// `<state_root>/identity_state/`.
///
/// Construct with [`IdentityStateStore::new`] over a resolved state root, then
/// [`load`](Self::load) / [`save`](Self::save) / [`load_or_seed`](Self::load_or_seed)
/// arbitrary serde payloads. The store creates directories lazily on the first
/// write and never on read.
#[derive(Clone, Debug)]
pub struct IdentityStateStore {
    root: PathBuf,
}

impl IdentityStateStore {
    /// Build a store rooted at `<state_root>/identity_state/`. `state_root` is
    /// the durable state root (e.g. [`crate::state_root::simard_state_root`]);
    /// it is never created here.
    pub fn new(state_root: &Path) -> Self {
        Self {
            root: state_root.join(IDENTITY_STATE_SUBDIR),
        }
    }

    /// The on-disk path for one `(identity, key)` payload:
    /// `<state_root>/identity_state/<identity>/<key>.toml`. Both segments are
    /// validated with [`is_safe_segment`]; a malformed segment is a hard error
    /// (never silently coerced) so no caller can traverse out of the store.
    fn entry_path(&self, identity: &str, key: &str) -> SimardResult<PathBuf> {
        if !is_safe_segment(identity) {
            return Err(SimardError::PromptAssetRead {
                path: self.root.clone(),
                reason: format!("unsafe identity segment for identity-state store: {identity:?}"),
            });
        }
        if !is_safe_segment(key) {
            return Err(SimardError::PromptAssetRead {
                path: self.root.clone(),
                reason: format!("unsafe key segment for identity-state store: {key:?}"),
            });
        }
        Ok(self.root.join(identity).join(format!("{key}.toml")))
    }

    /// Load and deserialize the `(identity, key)` payload.
    ///
    /// Returns `Ok(None)` when no payload has been persisted yet (the file does
    /// not exist), `Ok(Some(value))` when it parses, and `Err` on an I/O fault
    /// or a corrupt (unparseable) payload — a corrupt file is surfaced, never
    /// silently treated as absent.
    pub fn load<T: DeserializeOwned>(&self, identity: &str, key: &str) -> SimardResult<Option<T>> {
        let path = self.entry_path(identity, key)?;
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let value =
                    toml::from_str::<T>(&raw).map_err(|e| SimardError::PromptAssetRead {
                        path: path.clone(),
                        reason: format!("parse identity-state payload failed: {e}"),
                    })?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SimardError::PromptAssetRead {
                path,
                reason: format!("read identity-state payload failed: {e}"),
            }),
        }
    }

    /// Serialize and persist the `(identity, key)` payload, creating the
    /// per-identity directory on demand. The write is atomic: the payload is
    /// written to a temp file in the same directory and renamed into place, so a
    /// crash mid-write never leaves a half-written payload that would later fail
    /// to parse.
    pub fn save<T: Serialize>(&self, identity: &str, key: &str, value: &T) -> SimardResult<()> {
        let path = self.entry_path(identity, key)?;
        let dir = path
            .parent()
            .expect("entry_path always has a parent under the store root");
        std::fs::create_dir_all(dir).map_err(|e| SimardError::PromptAssetRead {
            path: dir.to_path_buf(),
            reason: format!("create identity-state dir failed: {e}"),
        })?;
        let raw = toml::to_string_pretty(value).map_err(|e| SimardError::PromptAssetRead {
            path: path.clone(),
            reason: format!("serialize identity-state payload failed: {e}"),
        })?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, raw.as_bytes()).map_err(|e| SimardError::PromptAssetRead {
            path: tmp.clone(),
            reason: format!("write identity-state payload failed: {e}"),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| SimardError::PromptAssetRead {
            path: path.clone(),
            reason: format!("commit identity-state payload failed: {e}"),
        })?;
        Ok(())
    }

    /// Load the payload, or SEED it on first use.
    ///
    /// When a persisted payload exists it is authoritative and returned as-is —
    /// this is what makes a curated edit durable across deploys: the seed runs
    /// exactly once, and thereafter the identity's own mutable copy wins. When
    /// none exists, `seed()` is evaluated, persisted, and returned, so the first
    /// reader materializes the identity's default and every later reader (and
    /// every later deploy) sees the same, mutable, on-disk state.
    pub fn load_or_seed<T, F>(&self, identity: &str, key: &str, seed: F) -> SimardResult<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> T,
    {
        if let Some(existing) = self.load::<T>(identity, key)? {
            return Ok(existing);
        }
        let seeded = seed();
        self.save(identity, key, &seeded)?;
        Ok(seeded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        items: Vec<String>,
    }

    #[test]
    fn safe_segment_accepts_clean_names() {
        assert!(is_safe_segment("simard"));
        assert!(is_safe_segment("gastronome"));
        assert!(is_safe_segment("governed_repos"));
        assert!(is_safe_segment("v1.2-beta"));
    }

    #[test]
    fn safe_segment_rejects_traversal_and_separators() {
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment("."));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("a b"));
        assert!(!is_safe_segment("a\0b"));
        assert!(!is_safe_segment("../escape"));
    }

    #[test]
    fn load_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        let got: Option<Sample> = store.load("simard", "governed_repos").unwrap();
        assert!(got.is_none(), "no file yet → Ok(None), never an error");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        let value = Sample {
            items: vec!["a".into(), "b".into()],
        };
        store.save("simard", "governed_repos", &value).unwrap();
        let got: Sample = store.load("simard", "governed_repos").unwrap().unwrap();
        assert_eq!(got, value);
    }

    #[test]
    fn payload_lives_under_identity_state_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        store
            .save("simard", "governed_repos", &Sample { items: vec![] })
            .unwrap();
        let expected = dir
            .path()
            .join(IDENTITY_STATE_SUBDIR)
            .join("simard")
            .join("governed_repos.toml");
        assert!(
            expected.is_file(),
            "payload must land at <state_root>/identity_state/<identity>/<key>.toml, got missing {}",
            expected.display()
        );
    }

    #[test]
    fn load_or_seed_seeds_once_then_is_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());

        // First call seeds and persists.
        let seeded: Sample = store
            .load_or_seed("simard", "governed_repos", || Sample {
                items: vec!["seed".into()],
            })
            .unwrap();
        assert_eq!(seeded.items, vec!["seed".to_string()]);

        // A curated edit overrides the seed and MUST win on the next read even
        // though the seed closure would still produce the default — this is the
        // deploy-durability guarantee.
        store
            .save(
                "simard",
                "governed_repos",
                &Sample {
                    items: vec!["curated".into()],
                },
            )
            .unwrap();
        let after: Sample = store
            .load_or_seed("simard", "governed_repos", || Sample {
                items: vec!["seed".into()],
            })
            .unwrap();
        assert_eq!(
            after.items,
            vec!["curated".to_string()],
            "persisted curated state must survive; the seed runs at most once"
        );
    }

    #[test]
    fn per_identity_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        store
            .save(
                "simard",
                "governed_repos",
                &Sample {
                    items: vec!["s".into()],
                },
            )
            .unwrap();
        store
            .save(
                "gastronome",
                "menus",
                &Sample {
                    items: vec!["g".into()],
                },
            )
            .unwrap();
        let s: Sample = store.load("simard", "governed_repos").unwrap().unwrap();
        let g: Sample = store.load("gastronome", "menus").unwrap().unwrap();
        assert_eq!(s.items, vec!["s".to_string()]);
        assert_eq!(g.items, vec!["g".to_string()]);
        // Simard has no menus; different key + identity are independent.
        assert!(store.load::<Sample>("simard", "menus").unwrap().is_none());
    }

    #[test]
    fn unsafe_identity_or_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        assert!(store.load::<Sample>("../etc", "governed_repos").is_err());
        assert!(
            store
                .save("simard", "../../secret", &Sample { items: vec![] })
                .is_err()
        );
    }

    #[test]
    fn corrupt_payload_is_surfaced_not_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        let path = dir
            .path()
            .join(IDENTITY_STATE_SUBDIR)
            .join("simard")
            .join("governed_repos.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is = = not valid toml [[[").unwrap();
        assert!(
            store.load::<Sample>("simard", "governed_repos").is_err(),
            "a corrupt payload must be an Err, never silently Ok(None)"
        );
    }
}
