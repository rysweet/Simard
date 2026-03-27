use std::fs;
use std::path::{Path, PathBuf};

use simard::{FilePromptAssetStore, PromptAssetRef, PromptAssetStore, SimardError};
use uuid::Uuid;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("simard-prompt-assets-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn file_prompt_asset_store_loads_assets_within_root() {
    let temp_dir = TestDir::new();
    let asset_dir = temp_dir.path().join("simard");
    fs::create_dir_all(&asset_dir).expect("asset directory should be created");
    fs::write(asset_dir.join("engineer_system.md"), "You are Simard.")
        .expect("asset file should be written");

    let store = FilePromptAssetStore::new(temp_dir.path());
    let asset = store
        .load(&PromptAssetRef::new(
            "engineer-system",
            "simard/engineer_system.md",
        ))
        .expect("asset should load from within root");

    assert_eq!(asset.contents, "You are Simard.");
}

#[test]
fn file_prompt_asset_store_rejects_absolute_paths() {
    let temp_dir = TestDir::new();
    let store = FilePromptAssetStore::new(temp_dir.path());

    let error = store
        .load(&PromptAssetRef::new("engineer-system", "/etc/passwd"))
        .unwrap_err();

    assert_eq!(
        error,
        SimardError::InvalidPromptAssetPath {
            asset_id: "engineer-system".to_string(),
            path: PathBuf::from("/etc/passwd"),
            reason: "expected a relative path inside the configured prompt root".to_string(),
        }
    );
}

#[test]
fn file_prompt_asset_store_rejects_parent_directory_traversal() {
    let temp_dir = TestDir::new();
    let store = FilePromptAssetStore::new(temp_dir.path());

    let error = store
        .load(&PromptAssetRef::new("engineer-system", "../secrets.txt"))
        .unwrap_err();

    assert_eq!(
        error,
        SimardError::InvalidPromptAssetPath {
            asset_id: "engineer-system".to_string(),
            path: PathBuf::from("../secrets.txt"),
            reason: "path traversal is not allowed".to_string(),
        }
    );
}

#[cfg(unix)]
#[test]
fn file_prompt_asset_store_rejects_symlinks_that_escape_root() {
    use std::os::unix::fs::symlink;

    let temp_dir = TestDir::new();
    let outside_path = temp_dir.path().join("outside-secret.txt");
    fs::write(&outside_path, "secret").expect("outside file should be written");

    let asset_dir = temp_dir.path().join("simard");
    fs::create_dir_all(&asset_dir).expect("asset directory should be created");
    symlink(&outside_path, asset_dir.join("escaped.md")).expect("symlink should be created");

    let store = FilePromptAssetStore::new(&asset_dir);
    let error = store
        .load(&PromptAssetRef::new("engineer-system", "escaped.md"))
        .unwrap_err();

    assert_eq!(
        error,
        SimardError::InvalidPromptAssetPath {
            asset_id: "engineer-system".to_string(),
            path: PathBuf::from("escaped.md"),
            reason: "path escapes configured prompt root".to_string(),
        }
    );
}
