//! Data-driven loader for EXAMPLE non-engineering identity packages.
//!
//! Example identities (gastronome, cartographer, bursar, …) are EXAMPLES of
//! what the pluggable-identity framework can produce. They live as data-only
//! packages under `examples/identities/<name>/` (identity.toml + prompts/ +
//! recipes/) and are loaded via the EXISTING [`FileIdentityLoader`]. They are
//! NEVER compiled into [`BuiltinIdentityLoader`], never add domain Rust to
//! `src/`, and never register an operator_cli subcommand or `src/bin/*`.
//!
//! This module is the thin rail that resolves a package directory
//! `<base_dir>/<name>/identity.toml` and delegates to that loader. It is the
//! ONLY `src/` footprint an example identity requires.

use std::path::Path;

use super::{FileIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest};
use crate::error::{SimardError, SimardResult};

/// Default repository-relative home for example identity packages.
pub const DEFAULT_EXAMPLE_IDENTITIES_DIR: &str = "examples/identities";

/// Validate `name` as a single, safe path segment BEFORE any filesystem
/// access. Mirrors the identity-name rule used by the file loader: non-empty,
/// ASCII, and only alphanumeric characters or hyphens. This rejects `..`,
/// `a/b`, `/etc/passwd`, and empty names, so `name` can never traverse out of
/// `base_dir`.
fn validate_example_name(base_dir: &Path, name: &str) -> SimardResult<()> {
    let is_valid = !name.is_empty()
        && name.is_ascii()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if is_valid {
        return Ok(());
    }
    Err(SimardError::IdentityTomlParseError {
        path: base_dir.join(name),
        reason: format!(
            "invalid example identity name '{name}': must be a single path segment of ASCII \
             alphanumeric characters and hyphens"
        ),
    })
}

/// Load an EXAMPLE identity package from `<base_dir>/<name>/identity.toml`
/// via the existing [`FileIdentityLoader`], with ZERO edits to
/// [`BuiltinIdentityLoader`].
///
/// Behavior contract (specified by the tests below):
/// - Validates `name` as a single path segment BEFORE touching the
///   filesystem (defense against path traversal via `name`).
/// - Requires `<base_dir>/<name>/identity.toml` to exist; a missing package
///   dir or manifest returns a clear [`SimardError::IdentityTomlParseError`]
///   (fail-visible, never a panic, never a silent fallback to a builtin).
/// - Delegates parsing/containment to [`FileIdentityLoader`] with both
///   `identity_path` and `prompt_root` set to the package directory so prompt
///   asset refs resolve as clean `prompts/*.md` and cannot escape the package.
pub fn load_example_identity(
    base_dir: &Path,
    name: &str,
    request: &IdentityLoadRequest,
) -> SimardResult<IdentityManifest> {
    // Reject a traversal/invalid `name` before touching the filesystem.
    validate_example_name(base_dir, name)?;

    let package_dir = base_dir.join(name);
    let manifest_path = package_dir.join("identity.toml");

    // Require the manifest to exist. The underlying FileIdentityLoader silently
    // falls back to BuiltinIdentityLoader on a missing identity.toml; an example
    // package must instead fail visibly, so we pre-check here.
    if !manifest_path.is_file() {
        return Err(SimardError::IdentityTomlParseError {
            path: manifest_path,
            reason: format!(
                "example identity package '{name}' not found: no identity.toml under '{}'",
                package_dir.display()
            ),
        });
    }

    // Delegate parsing + containment to the existing file loader. Setting both
    // identity_path and prompt_root to the package directory makes prompt-asset
    // refs resolve as clean `prompts/*.md` and prevents them from escaping the
    // package.
    FileIdentityLoader::new(&package_dir, &package_dir).load(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SimardError;
    use crate::identity::{BuiltinIdentityLoader, IdentityLoader, ManifestContract, OperatingMode};
    use crate::metadata::{Freshness, Provenance};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

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

    fn test_request(identity: &str) -> IdentityLoadRequest {
        IdentityLoadRequest::new(identity, "0.1.0", test_contract())
    }

    /// Write a minimal example identity package under `<base>/<name>/`.
    fn write_example_package(base: &Path, name: &str, identity_toml: &str) -> PathBuf {
        let pkg_dir = base.join(name);
        fs::create_dir_all(pkg_dir.join("prompts")).unwrap();
        fs::write(pkg_dir.join("identity.toml"), identity_toml).unwrap();
        pkg_dir
    }

    // ── Happy path: data-driven load, NO BuiltinIdentityLoader entry ────

    #[test]
    fn loads_example_package_from_toml() {
        let base = TempDir::new().unwrap();
        let name = "gastronome-example";
        let toml = r#"
[package]
name = "gastronome-example"
version = "0.1.0"
description = "An EXAMPLE non-engineering identity"

[[identities]]
name = "gastronome-example"
default_mode = "curator"

[[identities.prompt_assets]]
id = "gastronome-system"
path = "prompts/gastronome_system.md"
"#;
        let pkg_dir = write_example_package(base.path(), name, toml);
        fs::write(
            pkg_dir.join("prompts/gastronome_system.md"),
            "# Gastronome system prompt",
        )
        .unwrap();

        let manifest = load_example_identity(base.path(), name, &test_request(name)).unwrap();

        assert_eq!(manifest.name, "gastronome-example");
        assert_eq!(manifest.default_mode, OperatingMode::Curator);
        assert_eq!(manifest.prompt_assets.len(), 1);
        let asset = &manifest.prompt_assets[0];
        assert_eq!(asset.id.as_str(), "gastronome-system");
        assert_eq!(
            asset.relative_path,
            PathBuf::from("prompts/gastronome_system.md"),
            "prompt asset must resolve relative to the package dir (prompt_root)"
        );

        // Prove this is DATA-DRIVEN, not compiled in: the BuiltinIdentityLoader
        // must NOT know this example identity.
        let builtin_err = BuiltinIdentityLoader.load(&test_request(name)).unwrap_err();
        let _ = builtin_err; // any error is fine; the point is it is NOT loadable via builtin
    }

    // ── Fail-visible: missing package / manifest ────────────────────────

    #[test]
    fn missing_package_returns_error_not_panic() {
        let base = TempDir::new().unwrap();
        // No `<base>/nonexistent/` dir at all.
        let err = load_example_identity(base.path(), "nonexistent", &test_request("nonexistent"))
            .expect_err("missing example package must return a clear error, not fall back");
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "missing package must be a fail-visible IdentityTomlParseError, got: {err:?}"
        );
    }

    #[test]
    fn missing_manifest_in_existing_dir_returns_error() {
        let base = TempDir::new().unwrap();
        // Package dir exists but has no identity.toml.
        fs::create_dir_all(base.path().join("halfbaked")).unwrap();
        let err = load_example_identity(base.path(), "halfbaked", &test_request("halfbaked"))
            .expect_err("missing identity.toml must return an error, not a builtin fallback");
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "missing manifest must be a fail-visible IdentityTomlParseError, got: {err:?}"
        );
    }

    // ── Fail-visible: invalid manifest ──────────────────────────────────

    #[test]
    fn invalid_toml_returns_error() {
        let base = TempDir::new().unwrap();
        write_example_package(base.path(), "broken", "this is not valid TOML {{{}}}");
        let err = load_example_identity(base.path(), "broken", &test_request("broken"))
            .expect_err("malformed identity.toml must return an error");
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "invalid TOML must be an IdentityTomlParseError, got: {err:?}"
        );
    }

    // ── Security: path traversal via `name` (before any fs access) ───────

    #[test]
    fn traversal_name_rejected_without_fs_access() {
        let base = TempDir::new().unwrap();
        for bad in ["../evil", "..", "a/b", "/etc/passwd", ""] {
            let err = load_example_identity(base.path(), bad, &test_request("x"))
                .unwrap_err_or_else_panic(bad);
            assert!(
                matches!(err, SimardError::IdentityTomlParseError { .. }),
                "traversal/invalid name {bad:?} must be rejected as IdentityTomlParseError, got: {err:?}"
            );
        }
    }

    // ── Security: path traversal via TOML prompt-asset `path` ───────────

    #[test]
    fn toml_asset_path_traversal_rejected() {
        let base = TempDir::new().unwrap();
        let name = "escaper";
        let toml = r#"
[package]
name = "escaper"
version = "0.1.0"

[[identities]]
name = "escaper"
default_mode = "curator"

[[identities.prompt_assets]]
id = "escape"
path = "../../secret.md"
"#;
        write_example_package(base.path(), name, toml);
        let err = load_example_identity(base.path(), name, &test_request(name))
            .expect_err("prompt asset path escaping the package must be rejected");
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "asset path traversal must be an IdentityTomlParseError, got: {err:?}"
        );
    }

    // ── Reference package: cartographer parses + loads end-to-end ───────

    #[test]
    fn cartographer_parses_and_loads() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let base = repo_root.join(DEFAULT_EXAMPLE_IDENTITIES_DIR);
        let manifest = load_example_identity(&base, "cartographer", &test_request("cartographer"))
            .expect("the reference examples/identities/cartographer package must load");
        assert_eq!(manifest.name, "cartographer");
        assert_eq!(manifest.default_mode, OperatingMode::Curator);
        assert_eq!(
            manifest.prompt_assets.len(),
            5,
            "cartographer ships 5 phase prompts (system + explore + visualize + narrative + deliver)"
        );
    }

    // ── Small helper: turn a Result into its Err, panicking with context ─
    trait UnwrapErrOrPanic<T> {
        fn unwrap_err_or_else_panic(self, ctx: &str) -> SimardError;
    }
    impl UnwrapErrOrPanic<IdentityManifest> for SimardResult<IdentityManifest> {
        fn unwrap_err_or_else_panic(self, ctx: &str) -> SimardError {
            match self {
                Ok(_) => panic!("expected error for input {ctx:?} but got Ok"),
                Err(e) => e,
            }
        }
    }
}
