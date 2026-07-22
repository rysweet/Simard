//! Generic identity-scoped, mutable, deploy-durable **curated state**.
//!
//! An identity is more than the framework it runs on: like the example
//! identities (a Gastronome carries menus and events), Simard carries the set of
//! repositories she stewards. That "who am I" data should be *hers to curate*,
//! not a committed framework file that every self-deploy clobbers.
//!
//! This module is the framework's answer: a small, **generic** store where each
//! identity owns named **datasets** of curated items. The framework knows
//! nothing about "rosters" or "menus" — it only stores ordered, value-deduped
//! lists of `(value, note)` pairs. The governed-repo roster is *one* dataset
//! (`governed_repos` for the `simard` identity); a Gastronome's menu would be
//! another, differently-typed dataset owned by a different identity.
//!
//! ## Durability
//!
//! Datasets live under the durable **state root**
//! (`<state_root>/state/identity_state/<identity>/<dataset>.toml`). `install`
//! replaces only `~/.simard/{bin,prompt_assets,systemd}` and never touches the
//! state root (see `docs/reference/state-root-resolution.md`), so an identity's
//! curated edits **survive every self-deploy** — exactly what a git-tracked
//! `prompt_assets` file could never do.
//!
//! ## Seeding
//!
//! [`CuratedDataStore::load_or_seed`] populates a dataset from a caller-provided
//! seed on first use, then persists it. Thereafter the durable copy is the
//! single source of truth: the seed is only the *initial* value, and later
//! curation (add/remove) is never overwritten by a redeploy.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// The store label used in [`SimardError::PersistentStoreIo`] diagnostics.
const STORE_LABEL: &str = "identity-curated-state";

/// The durable subdirectory (under `<state_root>/state`) that holds every
/// identity's curated datasets. A hardcoded constant — never derived from env,
/// argv, or file contents.
const IDENTITY_STATE_SUBDIR: &str = "identity_state";

/// The current on-disk schema version for a [`CuratedList`].
pub const CURATED_STATE_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    CURATED_STATE_SCHEMA_VERSION
}

/// One curated item: an opaque `value` plus a human-readable `note`.
///
/// Deliberately generic — a governed `owner/name` repo slug for Simard, a dish
/// for a Gastronome menu, a watched developer handle for a research identity.
/// The store never interprets either field; only the caller gives them meaning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CuratedItem {
    /// The opaque identifying value (the roster slug, the dish name, …).
    pub value: String,
    /// A human-readable note. Ignored by the store; carried for the curator.
    #[serde(default)]
    pub note: String,
}

impl CuratedItem {
    /// Construct a curated item from a value and a note.
    pub fn new(value: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            note: note.into(),
        }
    }
}

/// An ordered, value-deduplicated list of curated items — one identity-scoped,
/// mutable, deploy-durable dataset.
///
/// Order is insertion order; duplicates (by `value`) are collapsed to the first
/// occurrence so a dataset can never steward the same value twice.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CuratedList {
    /// On-disk schema version, for forward-compatible migrations.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The curated items, in curation order. Serialized as a TOML `[[item]]`
    /// array so a human curator can read and hand-edit the durable file.
    #[serde(default, rename = "item")]
    pub items: Vec<CuratedItem>,
}

impl Default for CuratedList {
    fn default() -> Self {
        Self {
            schema_version: CURATED_STATE_SCHEMA_VERSION,
            items: Vec::new(),
        }
    }
}

impl CuratedList {
    /// An empty dataset at the current schema version.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a dataset from items, collapsing duplicate `value`s to the first
    /// occurrence while preserving order.
    pub fn from_items(items: impl IntoIterator<Item = CuratedItem>) -> Self {
        let mut list = Self::new();
        for item in items {
            list.add(item.value, item.note);
        }
        list
    }

    /// The ordered `value`s of every item (notes dropped).
    pub fn values(&self) -> Vec<String> {
        self.items.iter().map(|i| i.value.clone()).collect()
    }

    /// Whether an item with this `value` is present.
    pub fn contains(&self, value: &str) -> bool {
        self.items.iter().any(|i| i.value == value)
    }

    /// Add an item. Returns `true` if inserted, `false` if an item with the same
    /// `value` was already present (in which case the list is unchanged).
    pub fn add(&mut self, value: impl Into<String>, note: impl Into<String>) -> bool {
        let value = value.into();
        if self.contains(&value) {
            return false;
        }
        self.items.push(CuratedItem::new(value, note));
        true
    }

    /// Remove the item with this `value`. Returns `true` if one was removed.
    pub fn remove(&mut self, value: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.value != value);
        self.items.len() != before
    }

    /// Whether the dataset has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// A durable store of identity-scoped curated datasets rooted at a directory.
///
/// Production resolves the root under the state root via [`CuratedDataStore::resolve`];
/// tests use [`CuratedDataStore::with_root`] against a `tempdir` so they never
/// touch the ambient `~/.simard`.
#[derive(Clone, Debug)]
pub struct CuratedDataStore {
    root: PathBuf,
}

impl CuratedDataStore {
    /// The production store: `<state_root>/state/identity_state`.
    ///
    /// The state root is resolved by [`crate::state_root::simard_state_root`],
    /// which honors `SIMARD_STATE_ROOT`. `install` never overwrites it, so the
    /// curated datasets are deploy-durable.
    pub fn resolve() -> Self {
        Self::with_root(
            crate::state_root::simard_state_root()
                .join("state")
                .join(IDENTITY_STATE_SUBDIR),
        )
    }

    /// A store rooted at an explicit directory (test seam).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The durable file path for `<identity>/<dataset>.toml`, after validating
    /// both segments as safe (no separators, no `.`/`..`, no NUL, non-empty) so
    /// a caller-supplied identity or dataset name can never traverse the root.
    pub fn dataset_path(&self, identity: &str, dataset: &str) -> SimardResult<PathBuf> {
        validate_segment(identity, "identity")?;
        validate_segment(dataset, "dataset")?;
        Ok(self.root.join(identity).join(format!("{dataset}.toml")))
    }

    /// Load a dataset, or `Ok(None)` when it does not yet exist. An unreadable or
    /// malformed file is an `Err` (never silently treated as absent), so a
    /// caller can fail loud instead of re-seeding over corrupt curation.
    pub fn load(&self, identity: &str, dataset: &str) -> SimardResult<Option<CuratedList>> {
        let path = self.dataset_path(identity, dataset)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(SimardError::PersistentStoreIo {
                    store: STORE_LABEL.to_string(),
                    action: "read curated dataset".to_string(),
                    path,
                    reason: e.to_string(),
                });
            }
        };
        let list: CuratedList =
            toml::from_str(&raw).map_err(|e| SimardError::PersistentStoreIo {
                store: STORE_LABEL.to_string(),
                action: "parse curated dataset".to_string(),
                path,
                reason: e.to_string(),
            })?;
        Ok(Some(list))
    }

    /// Persist a dataset atomically (temp file + `rename`), creating the
    /// identity directory if needed.
    pub fn save(&self, identity: &str, dataset: &str, list: &CuratedList) -> SimardResult<()> {
        let path = self.dataset_path(identity, dataset)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: STORE_LABEL.to_string(),
                action: "create curated dataset dir".to_string(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }
        let body = toml::to_string_pretty(list).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_LABEL.to_string(),
            action: "serialize curated dataset".to_string(),
            path: path.clone(),
            reason: e.to_string(),
        })?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body.as_bytes()).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_LABEL.to_string(),
            action: "write curated dataset temp file".to_string(),
            path: tmp.clone(),
            reason: e.to_string(),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_LABEL.to_string(),
            action: "rename curated dataset into place".to_string(),
            path: path.clone(),
            reason: e.to_string(),
        })?;
        Ok(())
    }

    /// Load a dataset, or seed-and-persist `seed` when it does not yet exist,
    /// returning the durable list.
    ///
    /// This is the seeding-from-identity primitive: the first time an identity
    /// needs a dataset it is written from the seed (e.g. an identity's declared
    /// data), and every subsequent call returns the *curated* durable copy — so
    /// later add/remove edits are never reverted to the seed by a redeploy.
    pub fn load_or_seed(
        &self,
        identity: &str,
        dataset: &str,
        seed: &CuratedList,
    ) -> SimardResult<CuratedList> {
        if let Some(existing) = self.load(identity, dataset)? {
            return Ok(existing);
        }
        self.save(identity, dataset, seed)?;
        Ok(seed.clone())
    }
}

/// Validate a path segment (an identity or dataset name) as safe: non-empty, no
/// path separators, no `.`/`..`, no NUL. Keeps a caller-supplied name from ever
/// escaping the store root (path-traversal prevention).
fn validate_segment(segment: &str, kind: &str) -> SimardResult<()> {
    let invalid = |reason: &str| SimardError::PersistentStoreIo {
        store: STORE_LABEL.to_string(),
        action: format!("validate {kind} segment"),
        path: PathBuf::from(segment),
        reason: reason.to_string(),
    };
    if segment.is_empty() {
        return Err(invalid("segment must not be empty"));
    }
    if segment == "." || segment == ".." {
        return Err(invalid("segment must not be '.' or '..'"));
    }
    if segment.contains('/') || segment.contains('\\') {
        return Err(invalid("segment must not contain a path separator"));
    }
    if segment.contains('\0') {
        return Err(invalid("segment must not contain a NUL byte"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CuratedDataStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CuratedDataStore::with_root(dir.path().join("identity_state"));
        (dir, store)
    }

    // ── CuratedList ──────────────────────────────────────────────────────

    #[test]
    fn from_items_dedups_by_value_preserving_first_order() {
        let list = CuratedList::from_items(vec![
            CuratedItem::new("a", "first"),
            CuratedItem::new("b", "second"),
            CuratedItem::new("a", "dup ignored"),
        ]);
        assert_eq!(list.values(), vec!["a".to_string(), "b".to_string()]);
        // The first note wins; the duplicate is dropped whole.
        assert_eq!(list.items[0].note, "first");
    }

    #[test]
    fn add_is_idempotent_by_value() {
        let mut list = CuratedList::new();
        assert!(list.add("x", "note"));
        assert!(!list.add("x", "other"), "duplicate value is not re-added");
        assert_eq!(list.len(), 1);
        assert!(list.contains("x"));
    }

    #[test]
    fn remove_reports_whether_present() {
        let mut list = CuratedList::from_items(vec![CuratedItem::new("x", "")]);
        assert!(list.remove("x"));
        assert!(!list.remove("x"), "removing an absent value returns false");
        assert!(list.is_empty());
    }

    // ── round-trip / persistence ─────────────────────────────────────────

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, store) = store();
        let list = CuratedList::from_items(vec![
            CuratedItem::new("rysweet/Simard", "steward"),
            CuratedItem::new("rysweet/azlin", "azure vm cli"),
        ]);
        store.save("simard", "governed_repos", &list).unwrap();
        let loaded = store
            .load("simard", "governed_repos")
            .unwrap()
            .expect("saved dataset must load");
        assert_eq!(loaded, list);
    }

    #[test]
    fn load_absent_dataset_is_none_not_error() {
        let (_dir, store) = store();
        assert!(store.load("simard", "governed_repos").unwrap().is_none());
    }

    #[test]
    fn load_corrupt_dataset_is_error_not_none() {
        let (_dir, store) = store();
        let path = store.dataset_path("simard", "governed_repos").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is = = not valid toml").unwrap();
        assert!(
            store.load("simard", "governed_repos").is_err(),
            "a corrupt dataset is an error, never a silent empty/absent"
        );
    }

    // ── seeding (the identity-seed primitive) ────────────────────────────

    #[test]
    fn load_or_seed_persists_seed_on_first_use() {
        let (_dir, store) = store();
        let seed = CuratedList::from_items(vec![CuratedItem::new("rysweet/Simard", "steward")]);
        let first = store
            .load_or_seed("simard", "governed_repos", &seed)
            .unwrap();
        assert_eq!(first, seed, "first use returns the seed");
        // It is now durable on disk.
        assert!(
            store
                .dataset_path("simard", "governed_repos")
                .unwrap()
                .exists()
        );
        let reloaded = store.load("simard", "governed_repos").unwrap().unwrap();
        assert_eq!(reloaded, seed);
    }

    #[test]
    fn load_or_seed_returns_curated_copy_not_seed_after_mutation() {
        let (_dir, store) = store();
        let seed = CuratedList::from_items(vec![CuratedItem::new("rysweet/Simard", "steward")]);
        // Seed once.
        store
            .load_or_seed("simard", "governed_repos", &seed)
            .unwrap();
        // Curate: add a repo and remove the seeded one, then persist.
        let mut curated = store.load("simard", "governed_repos").unwrap().unwrap();
        curated.add("rysweet/azlin", "azure vm cli");
        curated.remove("rysweet/Simard");
        store.save("simard", "governed_repos", &curated).unwrap();
        // A subsequent load_or_seed with the SAME seed must return the curated
        // durable copy, never re-seed over it — this is the deploy-durability
        // guarantee (a redeploy that calls load_or_seed does not clobber edits).
        let after = store
            .load_or_seed("simard", "governed_repos", &seed)
            .unwrap();
        assert_eq!(after.values(), vec!["rysweet/azlin".to_string()]);
    }

    #[test]
    fn datasets_are_isolated_per_identity() {
        let (_dir, store) = store();
        let simard = CuratedList::from_items(vec![CuratedItem::new("rysweet/Simard", "repo")]);
        let gastronome = CuratedList::from_items(vec![CuratedItem::new("coq au vin", "dish")]);
        store.save("simard", "governed_repos", &simard).unwrap();
        store.save("gastronome", "menu", &gastronome).unwrap();
        assert_eq!(
            store.load("simard", "governed_repos").unwrap().unwrap(),
            simard
        );
        assert_eq!(
            store.load("gastronome", "menu").unwrap().unwrap(),
            gastronome
        );
        // Datasets do not bleed across identities.
        assert!(
            store
                .load("gastronome", "governed_repos")
                .unwrap()
                .is_none()
        );
    }

    // ── path safety ──────────────────────────────────────────────────────

    #[test]
    fn dataset_path_rejects_traversal_segments() {
        let (_dir, store) = store();
        assert!(store.dataset_path("..", "governed_repos").is_err());
        assert!(store.dataset_path("simard", "..").is_err());
        assert!(store.dataset_path("a/b", "governed_repos").is_err());
        assert!(store.dataset_path("simard", "a/b").is_err());
        assert!(store.dataset_path("", "governed_repos").is_err());
        assert!(store.dataset_path("simard", "").is_err());
    }

    #[test]
    fn dataset_path_is_under_root_for_clean_segments() {
        let (_dir, store) = store();
        let path = store.dataset_path("simard", "governed_repos").unwrap();
        assert!(path.starts_with(store.root()));
        assert!(path.ends_with("simard/governed_repos.toml"));
    }

    #[test]
    fn schema_version_defaults_when_absent_from_file() {
        // A hand-written dataset that omits schema_version still loads, taking
        // the current default — forward-compatible with a human curator's file.
        let (_dir, store) = store();
        let path = store.dataset_path("simard", "governed_repos").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[[item]]\nvalue = \"rysweet/Simard\"\nnote = \"steward\"\n",
        )
        .unwrap();
        let loaded = store.load("simard", "governed_repos").unwrap().unwrap();
        assert_eq!(loaded.schema_version, CURATED_STATE_SCHEMA_VERSION);
        assert_eq!(loaded.values(), vec!["rysweet/Simard".to_string()]);
    }
}
