//! Generic **identity-scoped mutable curated data** — durable, install-safe
//! state that belongs to *who an identity is*, not to the framework code.
//!
//! ## Why this exists
//! An identity should be separable from the framework the way the example
//! identities are: a Gastronome identity carries menus/events; Simard carries the
//! set of repos she stewards. That curated data must be:
//!
//! - **Mutable** — the identity curates it agentically at runtime (add/remove an
//!   item through its own reasoning), not by editing a committed source file.
//! - **Deploy-durable** — it lives under the durable state root
//!   ([`crate::state_root`]), which `install` never overwrites, so a self-deploy
//!   cannot clobber runtime curation (the failure mode that made a committed
//!   `prompt_assets/…` roster un-stewardable — every install re-installed it).
//! - **Generic** — the framework stores an opaque per-identity, per-collection
//!   TOML document and holds NO knowledge of what any collection *means*. Simard
//!   stores `governed_repos`; another identity could store `menus`. There is no
//!   hardcoded "ecosystem roster" concept in this layer.
//!
//! ## Layout
//! Each `(identity, collection)` pair is one TOML file:
//!
//! ```text
//! <state_root>/identity_state/<identity>/<collection>.toml
//! ```
//!
//! On first use a collection is **seeded** once from the identity's default
//! (e.g. an `include_str!`'d seed baked into the binary). Thereafter the on-disk
//! file is the single source of truth and the seed is never consulted again — the
//! identity owns and mutates its curated copy.
//!
//! ## Durability & safety
//! Writes go through [`crate::persistence::persist_bytes`] (temp-write → fsync →
//! atomic rename → parent-dir fsync, owner-only perms), so a reader never sees a
//! torn document and a crash never leaves a half-written collection. Identity and
//! collection names are validated as single, safe path segments
//! (`[A-Za-z0-9._-]`, no `.`/`..`/separators) so a name can never escape the
//! store root (path-traversal invariant), mirroring the roster slug validator.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{SimardError, SimardResult};

/// Store tag used for [`crate::error::SimardError::PersistentStoreIo`] attribution.
const STORE_TAG: &str = "identity_state";

/// The subdirectory (under the state root) that holds all identity-scoped
/// curated data. A compile-time constant — never derived from input.
const IDENTITY_STATE_SUBDIR: &str = "identity_state";

/// A durable, install-safe store of identity-scoped mutable curated data, rooted
/// at `<state_root>/identity_state/`.
///
/// The store is generic: it persists an opaque TOML document per
/// `(identity, collection)` and never interprets its contents. Callers own the
/// document shape (via `serde`), so different identities carry differently-typed
/// data with the same mechanism.
#[derive(Clone, Debug)]
pub struct IdentityStateStore {
    root: PathBuf,
}

impl IdentityStateStore {
    /// Open the store rooted under `state_root` (see [`crate::state_root`]). Does
    /// no I/O; the first writer creates directories.
    pub fn new(state_root: &Path) -> Self {
        Self {
            root: state_root.join(IDENTITY_STATE_SUBDIR),
        }
    }

    /// The on-disk path for a collection, or an error if `identity`/`collection`
    /// is not a safe single path segment (path-traversal prevention).
    pub fn collection_path(&self, identity: &str, collection: &str) -> SimardResult<PathBuf> {
        let identity = safe_segment(identity, "identity")?;
        let collection = safe_segment(collection, "collection")?;
        Ok(self.root.join(identity).join(format!("{collection}.toml")))
    }

    /// Load the raw TOML body of a collection, **seeding it on first use**.
    ///
    /// - If the collection file exists, its (possibly identity-curated) contents
    ///   are returned verbatim — this is the mutable source of truth.
    /// - If it does not exist, `seed` is written durably and returned, so the
    ///   identity's default becomes the initial curated state exactly once.
    pub fn load_or_seed_raw(
        &self,
        identity: &str,
        collection: &str,
        seed: &str,
    ) -> SimardResult<String> {
        let path = self.collection_path(identity, collection)?;
        match std::fs::read_to_string(&path) {
            Ok(body) => Ok(body),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::persistence::persist_bytes(STORE_TAG, &path, seed.as_bytes())?;
                Ok(seed.to_string())
            }
            Err(e) => Err(SimardError::PersistentStoreIo {
                store: STORE_TAG.to_string(),
                action: "read".to_string(),
                path,
                reason: e.to_string(),
            }),
        }
    }

    /// Persist a raw TOML body for a collection, atomically replacing any prior
    /// contents. This is how an identity commits an agentic curation edit.
    pub fn save_raw(&self, identity: &str, collection: &str, body: &str) -> SimardResult<()> {
        let path = self.collection_path(identity, collection)?;
        crate::persistence::persist_bytes(STORE_TAG, &path, body.as_bytes())
    }

    /// Typed convenience over [`Self::load_or_seed_raw`]: deserialize the curated
    /// collection (seeded from `seed_toml` on first use) into `T`.
    pub fn load_or_seed<T: DeserializeOwned>(
        &self,
        identity: &str,
        collection: &str,
        seed_toml: &str,
    ) -> SimardResult<T> {
        let raw = self.load_or_seed_raw(identity, collection, seed_toml)?;
        toml::from_str(&raw).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_TAG.to_string(),
            action: "deserialize".to_string(),
            path: self
                .collection_path(identity, collection)
                .unwrap_or_else(|_| self.root.clone()),
            reason: e.to_string(),
        })
    }

    /// Typed convenience over [`Self::save_raw`]: serialize `value` to TOML and
    /// persist it as the collection's new curated state.
    pub fn save<T: Serialize>(
        &self,
        identity: &str,
        collection: &str,
        value: &T,
    ) -> SimardResult<()> {
        let body = toml::to_string(value).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_TAG.to_string(),
            action: "serialize".to_string(),
            path: self
                .collection_path(identity, collection)
                .unwrap_or_else(|_| self.root.clone()),
            reason: e.to_string(),
        })?;
        self.save_raw(identity, collection, &body)
    }
}

/// Validate that `value` is a safe single path segment: non-empty, not `.`/`..`,
/// no path separators, and only `[A-Za-z0-9._-]`. Keeps identity/collection names
/// from escaping the store root. `kind` names the field for the error message.
fn safe_segment<'a>(value: &'a str, kind: &str) -> SimardResult<&'a str> {
    let ok = !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    if ok {
        Ok(value)
    } else {
        Err(SimardError::PersistentStoreIo {
            store: STORE_TAG.to_string(),
            action: "resolve".to_string(),
            path: PathBuf::from(value),
            reason: format!("invalid {kind} name for identity-scoped state: {value:?}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    const SEED: &str = "schema_version = 1\nvalue = \"seeded\"\n";

    #[test]
    fn seeds_on_first_use_then_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());

        // First use: file absent → seed is written and returned.
        let path = store.collection_path("simard", "governed_repos").unwrap();
        assert!(!path.exists(), "precondition: collection not yet seeded");
        let body = store
            .load_or_seed_raw("simard", "governed_repos", SEED)
            .unwrap();
        assert_eq!(body, SEED);
        assert!(path.exists(), "seed must be persisted to disk on first use");
    }

    #[test]
    fn second_load_returns_curated_copy_not_seed() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        store
            .load_or_seed_raw("simard", "governed_repos", SEED)
            .unwrap();

        // Identity curates the collection; a later load returns the CURATED copy,
        // and the seed is never consulted again (a different seed is ignored).
        store
            .save_raw("simard", "governed_repos", "curated = true\n")
            .unwrap();
        let body = store
            .load_or_seed_raw("simard", "governed_repos", SEED)
            .unwrap();
        assert_eq!(body, "curated = true\n");
    }

    #[test]
    fn collections_and_identities_are_namespaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        let a = store.collection_path("simard", "governed_repos").unwrap();
        let b = store.collection_path("gastronome", "menus").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("identity_state/simard/governed_repos.toml"));
        assert!(b.ends_with("identity_state/gastronome/menus.toml"));
    }

    #[test]
    fn generic_typed_roundtrip_for_arbitrary_shapes() {
        // The mechanism is generic: a different identity stores differently-typed
        // data (here, "menus") with the SAME store — no roster concept baked in.
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Menus {
            dishes: Vec<String>,
        }
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        let seed = "dishes = [\"soup\"]\n";
        let loaded: Menus = store.load_or_seed("gastronome", "menus", seed).unwrap();
        assert_eq!(loaded.dishes, vec!["soup".to_string()]);

        let updated = Menus {
            dishes: vec!["soup".into(), "salad".into()],
        };
        store.save("gastronome", "menus", &updated).unwrap();
        let reloaded: Menus = store.load_or_seed("gastronome", "menus", seed).unwrap();
        assert_eq!(reloaded, updated);
    }

    #[test]
    fn rejects_path_traversal_segments() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        for (identity, collection) in [
            ("..", "governed_repos"),
            ("simard", ".."),
            ("a/b", "c"),
            ("simard", "a/b"),
            ("", "c"),
            ("simard", ""),
            (".", "c"),
        ] {
            assert!(
                store.collection_path(identity, collection).is_err(),
                "unsafe segment ({identity:?}, {collection:?}) must be rejected"
            );
            assert!(
                store.load_or_seed_raw(identity, collection, SEED).is_err(),
                "load must reject unsafe segment ({identity:?}, {collection:?})"
            );
        }
    }

    #[test]
    fn corrupt_body_is_error_on_typed_load() {
        #[derive(Debug, Deserialize)]
        struct Doc {
            #[allow(dead_code)]
            n: u32,
        }
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStateStore::new(dir.path());
        store.save_raw("id", "col", "not valid = = toml").unwrap();
        assert!(store.load_or_seed::<Doc>("id", "col", "n = 1\n").is_err());
    }
}
