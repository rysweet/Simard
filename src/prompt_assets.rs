use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::Component;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PromptAssetId(String);

impl PromptAssetId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PromptAssetId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Display for PromptAssetId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptAssetRef {
    pub id: PromptAssetId,
    pub relative_path: PathBuf,
}

impl PromptAssetRef {
    pub fn new(id: impl Into<String>, relative_path: impl Into<PathBuf>) -> Self {
        Self {
            id: PromptAssetId::new(id),
            relative_path: relative_path.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptAsset {
    pub id: PromptAssetId,
    pub relative_path: PathBuf,
    pub contents: String,
}

impl PromptAsset {
    pub fn new(
        id: impl Into<String>,
        relative_path: impl Into<PathBuf>,
        contents: impl Into<String>,
    ) -> Self {
        Self {
            id: PromptAssetId::new(id),
            relative_path: relative_path.into(),
            contents: contents.into(),
        }
    }
}

pub trait PromptAssetStore: Send + Sync {
    fn load(&self, reference: &PromptAssetRef) -> SimardResult<PromptAsset>;
}

#[derive(Debug)]
pub struct FilePromptAssetStore {
    root: PathBuf,
}

impl FilePromptAssetStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl PromptAssetStore for FilePromptAssetStore {
    fn load(&self, reference: &PromptAssetRef) -> SimardResult<PromptAsset> {
        validate_prompt_asset_path(reference)?;

        let root = self
            .root
            .canonicalize()
            .map_err(|error| SimardError::PromptAssetRead {
                path: self.root.clone(),
                reason: error.to_string(),
            })?;
        let path = self.root.join(&reference.relative_path);
        let canonical_path = path.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SimardError::PromptAssetMissing {
                    asset_id: reference.id.to_string(),
                    path: path.clone(),
                }
            } else {
                SimardError::PromptAssetRead {
                    path: path.clone(),
                    reason: error.to_string(),
                }
            }
        })?;

        if !canonical_path.starts_with(&root) {
            return Err(SimardError::InvalidPromptAssetPath {
                asset_id: reference.id.to_string(),
                path: reference.relative_path.clone(),
                reason: "path escapes configured prompt root".to_string(),
            });
        }

        let contents =
            fs::read_to_string(&canonical_path).map_err(|error| SimardError::PromptAssetRead {
                path: canonical_path.clone(),
                reason: error.to_string(),
            })?;

        Ok(PromptAsset {
            id: reference.id.clone(),
            relative_path: reference.relative_path.clone(),
            contents,
        })
    }
}

fn validate_prompt_asset_path(reference: &PromptAssetRef) -> SimardResult<()> {
    if reference.relative_path.is_absolute() {
        return Err(SimardError::InvalidPromptAssetPath {
            asset_id: reference.id.to_string(),
            path: reference.relative_path.clone(),
            reason: "expected a relative path inside the configured prompt root".to_string(),
        });
    }

    if reference
        .relative_path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(SimardError::InvalidPromptAssetPath {
            asset_id: reference.id.to_string(),
            path: reference.relative_path.clone(),
            reason: "path traversal is not allowed".to_string(),
        });
    }

    Ok(())
}

#[derive(Debug, Default)]
pub struct InMemoryPromptAssetStore {
    assets: Mutex<BTreeMap<PromptAssetId, PromptAsset>>,
}

impl InMemoryPromptAssetStore {
    pub fn new(assets: impl IntoIterator<Item = PromptAsset>) -> Self {
        let map = assets
            .into_iter()
            .map(|asset| (asset.id.clone(), asset))
            .collect::<BTreeMap<_, _>>();
        Self {
            assets: Mutex::new(map),
        }
    }

    pub fn insert(&self, asset: PromptAsset) -> SimardResult<()> {
        self.assets
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "prompt_assets".to_string(),
            })?
            .insert(asset.id.clone(), asset);
        Ok(())
    }
}

impl PromptAssetStore for InMemoryPromptAssetStore {
    fn load(&self, reference: &PromptAssetRef) -> SimardResult<PromptAsset> {
        let assets = self
            .assets
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "prompt_assets".to_string(),
            })?;

        assets
            .get(&reference.id)
            .cloned()
            .ok_or_else(|| SimardError::PromptAssetMissing {
                asset_id: reference.id.to_string(),
                path: reference.relative_path.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_asset_id_new_and_as_str() {
        let id = PromptAssetId::new("test-asset");
        assert_eq!(id.as_str(), "test-asset");
    }

    #[test]
    fn test_prompt_asset_id_from_str() {
        let id: PromptAssetId = "my-id".into();
        assert_eq!(id.as_str(), "my-id");
    }

    #[test]
    fn test_prompt_asset_id_display() {
        let id = PromptAssetId::new("display-test");
        assert_eq!(format!("{id}"), "display-test");
    }

    #[test]
    fn test_prompt_asset_id_eq() {
        let a = PromptAssetId::new("same");
        let b = PromptAssetId::new("same");
        assert_eq!(a, b);
    }

    #[test]
    fn test_prompt_asset_id_ne() {
        let a = PromptAssetId::new("one");
        let b = PromptAssetId::new("two");
        assert_ne!(a, b);
    }

    #[test]
    fn test_prompt_asset_ref_new() {
        let r = PromptAssetRef::new("ref-id", "path/to/file.md");
        assert_eq!(r.id.as_str(), "ref-id");
        assert_eq!(r.relative_path, PathBuf::from("path/to/file.md"));
    }

    #[test]
    fn test_prompt_asset_new() {
        let asset = PromptAsset::new("a1", "file.md", "hello");
        assert_eq!(asset.id.as_str(), "a1");
        assert_eq!(asset.relative_path, PathBuf::from("file.md"));
        assert_eq!(asset.contents, "hello");
    }

    #[test]
    fn test_validate_prompt_asset_path_absolute_rejected() {
        let r = PromptAssetRef::new("abs", "/etc/passwd");
        let result = validate_prompt_asset_path(&r);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prompt_asset_path_traversal_rejected() {
        let r = PromptAssetRef::new("trav", "../secret.txt");
        let result = validate_prompt_asset_path(&r);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prompt_asset_path_valid() {
        let r = PromptAssetRef::new("ok", "prompts/system.md");
        let result = validate_prompt_asset_path(&r);
        assert!(result.is_ok());
    }

    #[test]
    fn test_in_memory_store_load_missing() {
        let store = InMemoryPromptAssetStore::default();
        let r = PromptAssetRef::new("missing", "file.md");
        let result = store.load(&r);
        assert!(result.is_err());
    }

    #[test]
    fn test_in_memory_store_insert_and_load() {
        let store = InMemoryPromptAssetStore::default();
        let asset = PromptAsset::new("a1", "file.md", "content");
        store.insert(asset.clone()).unwrap();

        let r = PromptAssetRef::new("a1", "file.md");
        let loaded = store.load(&r).unwrap();
        assert_eq!(loaded.contents, "content");
    }

    #[test]
    fn test_in_memory_store_new_with_assets() {
        let assets = vec![
            PromptAsset::new("a", "a.md", "aaa"),
            PromptAsset::new("b", "b.md", "bbb"),
        ];
        let store = InMemoryPromptAssetStore::new(assets);

        let r = PromptAssetRef::new("a", "a.md");
        assert_eq!(store.load(&r).unwrap().contents, "aaa");

        let r = PromptAssetRef::new("b", "b.md");
        assert_eq!(store.load(&r).unwrap().contents, "bbb");
    }

    #[test]
    fn test_file_store_load_missing_root() {
        let store = FilePromptAssetStore::new("/nonexistent/root/path");
        let r = PromptAssetRef::new("x", "file.md");
        let result = store.load(&r);
        assert!(result.is_err());
    }

    #[test]
    fn test_prompt_asset_id_ordering() {
        let a = PromptAssetId::new("alpha");
        let b = PromptAssetId::new("beta");
        assert!(a < b);
    }
}
