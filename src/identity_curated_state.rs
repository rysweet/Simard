//! Generic **identity-scoped mutable curated data** — deploy-durable state that
//! an identity OWNS and curates agentically, not a framework file bound to code.
//!
//! An identity carries its own differently-typed curated data: Simard stewards a
//! set of repos, a Gastronome identity would carry menus/events, a Bursar a
//! watchlist. This module is the GENERIC mechanism they share — it knows NOTHING
//! about repos or menus. Each *collection* is a named list of `{ key, note }`
//! items persisted as TOML under the **state root**, which `install` never
//! overwrites (unlike `prompt_assets/`, which every self-deploy re-installs from
//! the repo and would clobber runtime edits).
//!
//! ## Lifecycle: seed once from the identity, then own as mutable state
//!
//! [`load_or_seed`] is the entry point. On first use — when no durable file
//! exists yet — it SEEDS the collection from the identity (the caller's
//! `seed` closure, e.g. reading `identity.toml`'s declared data) and persists
//! it. Every later read returns the persisted, curated copy; the seed is never
//! consulted again, so [`add_item`] / [`remove_item`] edits survive re-installs
//! and re-deploys. This is the whole point: the roster (or menu, or watchlist)
//! becomes part of *who the identity is*, mutable and durable, not a committed
//! framework artifact.
//!
//! ## Layout
//!
//! `<state_root>/identity-state/<identity>/<collection>.toml`
//!
//! - `<state_root>` — [`crate::state_root::simard_state_root`] (honors
//!   `SIMARD_STATE_ROOT` / `SIMARD_HOME`); a `state_root_override` seam keeps
//!   tests hermetic against the ambient `~/.simard`.
//! - `identity-state` — a compile-time constant path segment (never derived from
//!   env, argv, or file contents).
//! - `<identity>` / `<collection>` — validated simple names (`[A-Za-z0-9._-]`,
//!   non-empty, no leading `-`, no `..`, no separators) so a hostile value can
//!   never traverse out of the identity-state tree.

use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// Compile-time constant path segment for the per-identity mutable-state tree.
/// Never derived from env, argv, or file contents (path-traversal invariant).
pub const IDENTITY_STATE_SUBDIR: &str = "identity-state";

/// The default identity name when none is selected via `SIMARD_IDENTITY` —
/// Simard herself. Curated data for the default identity lives under
/// `identity-state/simard/`.
pub const DEFAULT_IDENTITY: &str = "simard";

/// The store label carried in [`SimardError::PersistentStoreIo`] so a failure is
/// unambiguous in the journal.
const STORE_LABEL: &str = "identity_curated_state";

/// One curated item — a `key` plus a human-readable `note`. The interpretation
/// of both is entirely the consumer's: for Simard's `stewarded_repos` collection
/// the `key` is an `owner/name` repo slug; for another identity it might be a
/// dish name or a ticker. The mechanism stores strings and nothing more.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CuratedItem {
    pub key: String,
    #[serde(default)]
    pub note: String,
}

impl CuratedItem {
    pub fn new(key: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            note: note.into(),
        }
    }
}

/// A curated collection: a schema version plus the ordered `[[item]]` array.
/// Serializes to TOML as `schema_version = N` and repeated `[[item]]` tables.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CuratedCollection {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default, rename = "item")]
    pub items: Vec<CuratedItem>,
}

fn default_schema_version() -> u32 {
    1
}

impl Default for CuratedCollection {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            items: Vec::new(),
        }
    }
}

impl CuratedCollection {
    /// Build a collection from items at the current schema version.
    pub fn from_items(items: Vec<CuratedItem>) -> Self {
        Self {
            schema_version: default_schema_version(),
            items,
        }
    }

    /// The `key` of every item, in file order.
    pub fn keys(&self) -> Vec<String> {
        self.items.iter().map(|i| i.key.clone()).collect()
    }
}

/// Validate an identity or collection name as a single clean path segment.
/// Rejects anything that could traverse out of the identity-state tree or carry
/// a shell/path metacharacter: empty, a leading `-`, embedded `..`, a path
/// separator, or any character outside `[A-Za-z0-9._-]`.
fn is_simple_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Resolve the durable path for `<identity>`'s `<collection>` under the state
/// root: `<state_root>/identity-state/<identity>/<collection>.toml`.
///
/// `state_root_override` is a test seam; production passes `None`, which resolves
/// through [`crate::state_root::simard_state_root`]. Returns
/// [`SimardError::InvalidStateRoot`] if `identity` or `collection` is not a clean
/// single segment (path-traversal prevention) — the resolver performs no I/O.
pub fn resolve_curated_path(
    collection: &str,
    identity: &str,
    state_root_override: Option<&Path>,
) -> SimardResult<PathBuf> {
    for (label, value) in [("identity", identity), ("collection", collection)] {
        if !is_simple_name(value) {
            return Err(SimardError::InvalidStateRoot {
                path: PathBuf::from(value),
                reason: format!(
                    "identity-curated-state {label} name is not a clean single \
                     segment (only [A-Za-z0-9._-], no '..', no separators): {value:?}"
                ),
            });
        }
    }
    let root = state_root_override
        .map(PathBuf::from)
        .unwrap_or_else(crate::state_root::simard_state_root);
    Ok(root
        .join(IDENTITY_STATE_SUBDIR)
        .join(identity)
        .join(format!("{collection}.toml")))
}

/// Load the durable collection if it exists. Returns `Ok(None)` when no file has
/// been written yet (first use — the caller should seed). A present-but-corrupt
/// file is an `Err` (never silently treated as empty), so a caller relying on the
/// data fails loud instead of acting on a phantom empty set.
pub fn load(
    collection: &str,
    identity: &str,
    state_root_override: Option<&Path>,
) -> SimardResult<Option<CuratedCollection>> {
    let path = resolve_curated_path(collection, identity, state_root_override)?;
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| SimardError::PersistentStoreIo {
        store: STORE_LABEL.into(),
        action: "read".into(),
        path: path.clone(),
        reason: e.to_string(),
    })?;
    let parsed: CuratedCollection =
        toml::from_str(&raw).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_LABEL.into(),
            action: "parse".into(),
            path: path.clone(),
            reason: e.to_string(),
        })?;
    Ok(Some(parsed))
}

/// Persist a collection atomically (write a sibling `.tmp` then rename), creating
/// the `<state_root>/identity-state/<identity>/` directory tree as needed.
pub fn save(
    collection: &str,
    identity: &str,
    data: &CuratedCollection,
    state_root_override: Option<&Path>,
) -> SimardResult<()> {
    let path = resolve_curated_path(collection, identity, state_root_override)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
            store: STORE_LABEL.into(),
            action: "mkdir".into(),
            path: parent.to_path_buf(),
            reason: e.to_string(),
        })?;
    }
    let body = toml::to_string_pretty(data).map_err(|e| SimardError::PersistentStoreIo {
        store: STORE_LABEL.into(),
        action: "serialize".into(),
        path: path.clone(),
        reason: e.to_string(),
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body).map_err(|e| SimardError::PersistentStoreIo {
        store: STORE_LABEL.into(),
        action: "write".into(),
        path: tmp.clone(),
        reason: e.to_string(),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| SimardError::PersistentStoreIo {
        store: STORE_LABEL.into(),
        action: "rename".into(),
        path: path.clone(),
        reason: e.to_string(),
    })?;
    Ok(())
}

/// Load the durable collection, or — on first use — SEED it from the identity and
/// persist that seed before returning it.
///
/// This is the one-time handoff from framework-provided default to
/// identity-owned mutable state. The `seed` closure supplies the collection the
/// identity declares (e.g. from `identity.toml`); it is invoked ONLY when no
/// durable file exists. After the first seed the persisted copy is authoritative
/// and the seed is never consulted again — so subsequent [`add_item`] /
/// [`remove_item`] curation survives re-installs and re-deploys.
pub fn load_or_seed<F>(
    collection: &str,
    identity: &str,
    seed: F,
    state_root_override: Option<&Path>,
) -> SimardResult<CuratedCollection>
where
    F: FnOnce() -> SimardResult<CuratedCollection>,
{
    if let Some(existing) = load(collection, identity, state_root_override)? {
        return Ok(existing);
    }
    let seeded = seed()?;
    save(collection, identity, &seeded, state_root_override)?;
    Ok(seeded)
}

/// Add or update an item in the collection (upsert by `key`, preserving order:
/// an existing key keeps its position with the new note; a new key is appended),
/// then persist. Returns the updated collection. Curation primitive an identity
/// drives agentically to add a stewarded repo (or menu, or ticker).
pub fn add_item(
    collection: &str,
    identity: &str,
    item: CuratedItem,
    state_root_override: Option<&Path>,
) -> SimardResult<CuratedCollection> {
    let mut current = load(collection, identity, state_root_override)?.unwrap_or_default();
    if let Some(existing) = current.items.iter_mut().find(|i| i.key == item.key) {
        existing.note = item.note;
    } else {
        current.items.push(item);
    }
    save(collection, identity, &current, state_root_override)?;
    Ok(current)
}

/// Remove the item with `key` from the collection (a no-op if absent), then
/// persist. Returns the updated collection. The curation primitive an identity
/// drives to drop a stewarded repo.
pub fn remove_item(
    collection: &str,
    identity: &str,
    key: &str,
    state_root_override: Option<&Path>,
) -> SimardResult<CuratedCollection> {
    let mut current = load(collection, identity, state_root_override)?.unwrap_or_default();
    current.items.retain(|i| i.key != key);
    save(collection, identity, &current, state_root_override)?;
    Ok(current)
}

/// The active identity name for curated-state lookups: `SIMARD_IDENTITY` when set
/// and non-blank, else [`DEFAULT_IDENTITY`] (`"simard"`). A blank or unset var
/// means Simard herself, so her stewarded data lives under `identity-state/simard/`.
pub fn active_identity() -> String {
    match std::env::var("SIMARD_IDENTITY") {
        Ok(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => DEFAULT_IDENTITY.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn path_is_state_root_identity_state_identity_collection() {
        let root = tmp_root();
        let path = resolve_curated_path("stewarded_repos", "simard", Some(root.path())).unwrap();
        assert_eq!(
            path,
            root.path()
                .join("identity-state")
                .join("simard")
                .join("stewarded_repos.toml"),
        );
    }

    #[test]
    fn path_rejects_traversal_in_identity_or_collection() {
        let root = tmp_root();
        assert!(resolve_curated_path("stewarded_repos", "../etc", Some(root.path())).is_err());
        assert!(resolve_curated_path("..", "simard", Some(root.path())).is_err());
        assert!(resolve_curated_path("a/b", "simard", Some(root.path())).is_err());
        assert!(resolve_curated_path("stewarded_repos", "", Some(root.path())).is_err());
        assert!(resolve_curated_path("stewarded_repos", "-lead", Some(root.path())).is_err());
    }

    #[test]
    fn load_missing_is_none_not_error() {
        let root = tmp_root();
        assert_eq!(
            load("stewarded_repos", "simard", Some(root.path())).unwrap(),
            None,
            "no durable file yet → None (first use), never an error",
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let root = tmp_root();
        let data = CuratedCollection::from_items(vec![
            CuratedItem::new("rysweet/Simard", "steward"),
            CuratedItem::new("rysweet/azlin", "vm cli"),
        ]);
        save("stewarded_repos", "simard", &data, Some(root.path())).unwrap();
        let loaded = load("stewarded_repos", "simard", Some(root.path()))
            .unwrap()
            .unwrap();
        assert_eq!(loaded, data);
        assert_eq!(loaded.keys(), vec!["rysweet/Simard", "rysweet/azlin"]);
    }

    #[test]
    fn load_or_seed_seeds_once_then_owns_mutable_state() {
        let root = tmp_root();
        // First use seeds from the identity closure and persists it.
        let seeded = load_or_seed(
            "stewarded_repos",
            "simard",
            || {
                Ok(CuratedCollection::from_items(vec![CuratedItem::new(
                    "rysweet/Simard",
                    "steward",
                )]))
            },
            Some(root.path()),
        )
        .unwrap();
        assert_eq!(seeded.keys(), vec!["rysweet/Simard"]);

        // A later curation edit adds a repo to the OWNED mutable state.
        add_item(
            "stewarded_repos",
            "simard",
            CuratedItem::new("rysweet/azlin", "vm cli"),
            Some(root.path()),
        )
        .unwrap();

        // A subsequent load_or_seed must return the CURATED copy, never re-seed —
        // proving the edit survives (as it would across a re-install/re-deploy).
        let again = load_or_seed(
            "stewarded_repos",
            "simard",
            || panic!("seed must not be consulted once durable state exists"),
            Some(root.path()),
        )
        .unwrap();
        assert_eq!(again.keys(), vec!["rysweet/Simard", "rysweet/azlin"]);
    }

    #[test]
    fn add_item_upserts_and_preserves_order() {
        let root = tmp_root();
        add_item(
            "c",
            "id",
            CuratedItem::new("a/one", "first"),
            Some(root.path()),
        )
        .unwrap();
        add_item(
            "c",
            "id",
            CuratedItem::new("a/two", "second"),
            Some(root.path()),
        )
        .unwrap();
        // Update the note of an existing key — position is preserved, no dup.
        let after = add_item(
            "c",
            "id",
            CuratedItem::new("a/one", "updated"),
            Some(root.path()),
        )
        .unwrap();
        assert_eq!(after.keys(), vec!["a/one", "a/two"]);
        assert_eq!(after.items[0].note, "updated");
    }

    #[test]
    fn remove_item_drops_key_and_is_idempotent() {
        let root = tmp_root();
        add_item("c", "id", CuratedItem::new("a/one", ""), Some(root.path())).unwrap();
        add_item("c", "id", CuratedItem::new("a/two", ""), Some(root.path())).unwrap();
        let after = remove_item("c", "id", "a/one", Some(root.path())).unwrap();
        assert_eq!(after.keys(), vec!["a/two"]);
        // Removing an absent key is a no-op, not an error.
        let again = remove_item("c", "id", "a/one", Some(root.path())).unwrap();
        assert_eq!(again.keys(), vec!["a/two"]);
    }

    #[test]
    fn distinct_identities_and_collections_are_isolated() {
        let root = tmp_root();
        add_item(
            "stewarded_repos",
            "simard",
            CuratedItem::new("rysweet/Simard", ""),
            Some(root.path()),
        )
        .unwrap();
        add_item(
            "menus",
            "gastronome",
            CuratedItem::new("tasting-menu", ""),
            Some(root.path()),
        )
        .unwrap();
        // Each identity/collection sees only its own data (per-identity, per-type).
        assert_eq!(
            load("stewarded_repos", "simard", Some(root.path()))
                .unwrap()
                .unwrap()
                .keys(),
            vec!["rysweet/Simard"],
        );
        assert_eq!(
            load("menus", "gastronome", Some(root.path()))
                .unwrap()
                .unwrap()
                .keys(),
            vec!["tasting-menu"],
        );
        assert!(
            load("menus", "simard", Some(root.path()))
                .unwrap()
                .is_none(),
            "simard has no menus collection",
        );
    }

    #[test]
    fn corrupt_file_is_error_not_silent_empty() {
        let root = tmp_root();
        let path = resolve_curated_path("c", "id", Some(root.path())).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is = = not valid toml").unwrap();
        assert!(
            load("c", "id", Some(root.path())).is_err(),
            "a corrupt durable file fails loud, never a phantom empty set",
        );
    }
}
