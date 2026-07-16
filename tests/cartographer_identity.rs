//! Integration coverage for the `simard-cartographer` data-storytelling
//! identity: it resolves through the builtin loader, its system prompt asset
//! exists on disk and loads through the file-backed store, and the
//! `cartographer-dashboard` recipe plus the four stage prompts are present and
//! wired to each other.
//!
//! These are deterministic, Python-free, and run under the main `cargo test`
//! gate — they are the enforced equivalent of the outside-in gadugi scenario in
//! `tests/gadugi/cartographer-identity-assets.*`.

use std::fs;
use std::path::{Path, PathBuf};

use simard::{
    BaseTypeCapability, BaseTypeId, BuiltinIdentityLoader, FilePromptAssetStore, Freshness,
    IdentityLoadRequest, IdentityLoader, ManifestContract, OperatingMode, PromptAssetRef,
    PromptAssetStore, Provenance,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn prompt_assets_root() -> PathBuf {
    repo_root().join("prompt_assets")
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "test::entrypoint",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("test-source", "test-locator"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

#[test]
fn builtin_loader_resolves_cartographer_identity() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-cartographer",
            "0.1.0",
            test_contract(),
        ))
        .expect("cartographer identity should resolve through the builtin loader");

    assert_eq!(manifest.name, "simard-cartographer");
    // Delivery requires writing app files and serving a process, so Cartographer
    // runs in engineer mode with the engineer base-type reach.
    assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
    assert!(manifest.supports_base_type(&BaseTypeId::new("terminal-shell")));

    assert_eq!(manifest.prompt_assets.len(), 1);
    assert_eq!(manifest.prompt_assets[0].id.as_str(), "cartographer-system");
    assert_eq!(
        manifest.prompt_assets[0].relative_path,
        Path::new("simard/cartographer_system.md")
    );

    for capability in [
        BaseTypeCapability::PromptAssets,
        BaseTypeCapability::SessionLifecycle,
        BaseTypeCapability::Memory,
        BaseTypeCapability::Evidence,
        BaseTypeCapability::Reflection,
    ] {
        assert!(
            manifest.required_capabilities.contains(&capability),
            "cartographer should require {capability:?}"
        );
    }
}

#[test]
fn cartographer_system_prompt_loads_from_file_store() {
    let store = FilePromptAssetStore::new(prompt_assets_root());
    let asset = store
        .load(&PromptAssetRef::new(
            "cartographer-system",
            "simard/cartographer_system.md",
        ))
        .expect("cartographer system prompt should load from the prompt-asset store");

    // The system prompt must establish the identity and its core loop/mission.
    assert!(asset.contents.contains("Cartographer"));
    assert!(asset.contents.contains("inspect"));
    assert!(asset.contents.contains("narrative"));
    // Untrusted-data guard must be present.
    assert!(
        asset.contents.to_lowercase().contains("untrusted"),
        "system prompt must carry the untrusted-data guard"
    );
}

#[test]
fn cartographer_stage_prompts_exist_on_disk() {
    let root = prompt_assets_root().join("simard");
    for stage in [
        "cartographer_system.md",
        "cartographer_explore.md",
        "cartographer_visualize.md",
        "cartographer_deliver.md",
        "cartographer_narrative.md",
    ] {
        let path = root.join(stage);
        assert!(
            path.is_file(),
            "cartographer stage prompt {} should exist",
            path.display()
        );
    }
}

#[test]
fn cartographer_recipe_orchestrates_the_four_stages() {
    let recipe = prompt_assets_root().join("simard/recipes/cartographer-dashboard.yaml");
    let contents = fs::read_to_string(&recipe).unwrap_or_else(|e| {
        panic!(
            "cartographer recipe {} should be readable: {e}",
            recipe.display()
        )
    });

    // The recipe declares the identity's four stages in order.
    for step_id in [
        "explore",
        "design-visualizations",
        "deliver-dashboard",
        "write-narrative",
    ] {
        assert!(
            contents.contains(&format!("id: \"{step_id}\"")),
            "recipe should declare step '{step_id}'"
        );
    }

    // The recipe takes a dataset + a question and serves to a port — the
    // end-to-end contract of the identity.
    for var in ["dataset_path", "question", "output_dir", "serve_port"] {
        assert!(
            contents.contains(var),
            "recipe should thread context variable '{var}'"
        );
    }

    // Delivery must actually serve and verify a live URL, not just describe one.
    assert!(
        contents.contains("http://127.0.0.1:"),
        "delivery stage must fetch the served URL to verify it renders"
    );
    // The four named delivery stacks the identity ships for.
    for tool in ["Streamlit", "Plotly", "Observable", "D3"] {
        assert!(
            contents.contains(tool),
            "recipe should reference the {tool} delivery stack"
        );
    }
}
