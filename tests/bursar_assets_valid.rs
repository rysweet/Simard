//! Asset-level validation for the shipped Bursar EXAMPLE identity package.
//!
//! Bursar is a DATA-ONLY example identity (investment-portfolio research &
//! advisory, research/advisory ONLY — never order execution). It lives under
//! `examples/identities/bursar/` and is loaded by the data-driven
//! [`simard::identity::load_example_identity`] rail, NOT compiled into
//! `BuiltinIdentityLoader`. These tests prove the package's assets are not just
//! present but actually parse and load through the real Simard loader, and that
//! the recipe carries the domain + advisory-only contract that defines Bursar.
//!
//! This is an INTEGRATION test under `tests/` (never `src/`): it exercises the
//! example identity end-to-end via the public loader + its recipe, adding ZERO
//! Rust to `src/`.

use std::path::{Path, PathBuf};

use simard::identity::{
    DEFAULT_EXAMPLE_IDENTITIES_DIR, IdentityLoadRequest, ManifestContract, OperatingMode,
    WritePosture, load_example_identity,
};
use simard::metadata::{Freshness, Provenance};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn example_base() -> PathBuf {
    repo_path(DEFAULT_EXAMPLE_IDENTITIES_DIR)
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "bursar::assets::test",
        "portfolio + mandate -> allocation, backtest, risk, plan, report",
        vec!["test:bursar".to_string()],
        Provenance::new("bursar-assets-test", "tests/bursar_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

/// The example package must load via the data-driven rail as an advisory,
/// read-only, curator-mode identity carrying its full stage prompt set.
#[test]
fn bursar_example_identity_loads_read_only_curator_with_all_prompts() {
    let manifest = load_example_identity(&example_base(), "bursar", &test_request("bursar"))
        .expect("the examples/identities/bursar package must load via load_example_identity");

    assert_eq!(manifest.name, "bursar");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Curator,
        "bursar is a research/advisory curator, not an engineer"
    );
    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "bursar is advisory-only: it must load with a read-only write-authority posture"
    );

    // System prompt + five stage prompts (allocate, backtest, risk, rebalance,
    // report) — the loop the recipe orchestrates.
    let expected_asset_ids = [
        "bursar-system",
        "bursar-allocate",
        "bursar-backtest",
        "bursar-risk",
        "bursar-rebalance",
        "bursar-report",
    ];
    assert_eq!(
        manifest.prompt_assets.len(),
        expected_asset_ids.len(),
        "bursar ships {} phase prompts (system + allocate + backtest + risk + rebalance + report)",
        expected_asset_ids.len()
    );
    for id in expected_asset_ids {
        assert!(
            manifest.prompt_assets.iter().any(|a| a.id.as_str() == id),
            "bursar identity must expose the `{id}` prompt asset"
        );
    }

    // Every referenced prompt asset must resolve inside the package and be
    // non-empty on disk.
    let pkg_dir = example_base().join("bursar");
    for asset in &manifest.prompt_assets {
        let path = pkg_dir.join(&asset.relative_path);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("prompt asset {} must be readable: {e}", path.display()));
        assert!(
            body.trim().len() > 200,
            "prompt asset {} must carry real content",
            path.display()
        );
    }
}

/// The bursar identity name must be data-only: there is NO `BuiltinIdentityLoader`
/// arm for it (proving the package is discovered as data, not compiled in).
#[test]
fn bursar_is_not_a_builtin_identity() {
    use simard::identity::{BuiltinIdentityLoader, IdentityLoader};
    let err = BuiltinIdentityLoader
        .load(&test_request("bursar"))
        .expect_err("bursar must NOT be loadable via BuiltinIdentityLoader — it is data-only");
    // Any error is fine; the point is the builtin loader does not know it.
    let _ = err;
}

/// The goal-session recipe must drive the five-stage Bursar loop end-to-end and
/// carry the domain tooling + the advisory-only (no order execution) contract.
#[test]
fn bursar_recipe_drives_stages_tooling_and_advisory_only_contract() {
    let recipe_path = example_base().join("bursar/recipes/bursar-portfolio-review.yaml");
    let recipe = std::fs::read_to_string(&recipe_path)
        .unwrap_or_else(|e| panic!("recipe {} must be readable: {e}", recipe_path.display()));

    // Recipe skeleton.
    for required in ["name:", "description:", "version:", "steps:", "context:"] {
        assert!(
            recipe.contains(required),
            "recipe must contain `{required}`"
        );
    }

    // The five stages, in the inspect -> act -> verify -> persist loop.
    for stage in [
        "id: \"allocate\"",
        "id: \"backtest\"",
        "id: \"risk\"",
        "id: \"rebalance\"",
        "id: \"report\"",
    ] {
        assert!(recipe.contains(stage), "recipe must define stage `{stage}`");
    }

    // Context vars the stages consume.
    for var in ["portfolio_path", "prices_path", "mandate", "output_dir"] {
        assert!(
            recipe.contains(var),
            "recipe must expose the `{var}` context var"
        );
    }

    // Domain tooling lives in the recipe (agent sessions), never in src/.
    for tool in ["pandas", "backtrader", "QuantLib"] {
        assert!(
            recipe.contains(tool),
            "recipe must reference the `{tool}` domain tool the identity uses"
        );
    }

    // The defining guardrail: advisory / research ONLY, never order execution.
    let lower = recipe.to_lowercase();
    assert!(
        lower.contains("advisory") && lower.contains("never"),
        "recipe must state the advisory-only boundary"
    );
    assert!(
        lower.contains("execute") || lower.contains("order"),
        "recipe must explicitly address (and forbid) order execution"
    );
    assert!(
        lower.contains("untrusted data") || lower.contains("not instructions"),
        "recipe must instruct the agent to treat inputs as untrusted data, not instructions"
    );

    // A rebalancing PLAN, not a trade: the rebalance stage must forbid execution.
    assert!(
        lower.contains("refuse") && lower.contains("plan"),
        "the rebalance stage must produce a reviewable plan and refuse execution"
    );

    // Data-only package guarantee: no Rust source files anywhere in the package.
    let pkg_dir = example_base().join("bursar");
    assert!(
        !has_rust_source(&pkg_dir),
        "an example identity package must be data-only: no .rs files under {}",
        pkg_dir.display()
    );
}

/// Recurse `dir` and report whether any `*.rs` file exists under it.
fn has_rust_source(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if has_rust_source(&path) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            return true;
        }
    }
    false
}
