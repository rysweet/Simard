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

/// Derive the list of EXAMPLE identity package names from `base_dir`, purely
/// from the directory tree — the single source of truth for the self-maintaining
/// index (issue #4274). This has ZERO coupling to any specific identity name.
///
/// A subdirectory of `base_dir` is an example identity package **iff** it
/// contains an `identity.toml`. Loose files (e.g. the shared `README.md`) and
/// package-less directories are excluded. Each candidate directory name is
/// validated as a safe single path segment ([`validate_example_name`]); an
/// unsafe name is a HARD [`SimardError`] rather than a silent skip, so a
/// dropped or tampered package can never vanish from the derived index.
///
/// The returned names are sorted ascending (byte-wise), so the derivation — and
/// anything rendered from it — is deterministic and order-stable regardless of
/// filesystem enumeration order. Adding a new package is therefore a pure data
/// change: the new name simply appears in sorted position.
///
/// Fail-visible: any I/O failure while reading `base_dir` (including a missing
/// base directory) propagates as a [`SimardError`] — never a silent empty list.
pub fn list_example_identities(base_dir: &Path) -> SimardResult<Vec<String>> {
    let entries =
        std::fs::read_dir(base_dir).map_err(|source| SimardError::IdentityTomlParseError {
            path: base_dir.to_path_buf(),
            reason: format!(
                "failed to read example identities directory '{}': {source}",
                base_dir.display()
            ),
        })?;

    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SimardError::IdentityTomlParseError {
            path: base_dir.to_path_buf(),
            reason: format!(
                "failed to enumerate an entry in '{}': {source}",
                base_dir.display()
            ),
        })?;

        // Only real directories are package candidates. Use file_type() (a
        // symlink is NOT followed here) so a stray symlink cannot masquerade
        // as a package directory.
        let file_type =
            entry
                .file_type()
                .map_err(|source| SimardError::IdentityTomlParseError {
                    path: entry.path(),
                    reason: format!("failed to stat '{}': {source}", entry.path().display()),
                })?;
        if !file_type.is_dir() {
            continue;
        }

        // A directory is a package iff it holds an identity.toml.
        if !entry.path().join("identity.toml").is_file() {
            continue;
        }

        // The directory name must be a valid, safe identity name. An invalid
        // name is fail-visible, never silently skipped.
        let name = entry.file_name().to_string_lossy().into_owned();
        validate_example_name(base_dir, &name)?;
        names.push(name);
    }

    names.sort_unstable();
    Ok(names)
}

/// Render the derived example-identity index as the Markdown block body that
/// lives between the `BEGIN`/`END GENERATED IDENTITY INDEX` markers in
/// `examples/identities/README.md`.
///
/// Emits exactly one `- [<name>](./<name>/README.md)\n` line per package, in the
/// ascending order produced by [`list_example_identities`]. Each entry links to
/// that package's own `README.md` — the package is self-describing, so its blurb
/// is package-owned rather than centrally enumerated. The output is
/// deterministic and byte-stable; a staleness test asserts the committed block
/// equals this render, so the index can never drift.
///
/// Names are validated by [`list_example_identities`] to be ASCII alphanumeric
/// plus hyphens, which excludes every Markdown/HTML metacharacter — so a package
/// name can never inject link or HTML markup into the rendered index.
pub fn render_identity_index(base_dir: &Path) -> SimardResult<String> {
    let names = list_example_identities(base_dir)?;
    let mut out = String::new();
    for name in names {
        out.push_str(&format!("- [{name}](./{name}/README.md)\n"));
    }
    Ok(out)
}

/// Validate `name` as a single, safe path segment BEFORE any filesystem
/// access. Mirrors the identity-name rule used by the file loader: non-empty
/// and only ASCII alphanumeric characters or hyphens. This rejects `..`,
/// `a/b`, `/etc/passwd`, and empty names, so `name` can never traverse out of
/// `base_dir`.
fn validate_example_name(base_dir: &Path, name: &str) -> SimardResult<()> {
    // Single pass: `is_ascii_alphanumeric()` and `== '-'` are both ASCII-only,
    // so this predicate already implies `name.is_ascii()` — no separate scan.
    let is_valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
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
                .expect_err(&format!("traversal/invalid name {bad:?} must be rejected"));
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

    // ── Example packages: atelier + concierge parse, load, are NOT builtin ─
    //
    // These re-homed EXAMPLE identities (formerly wrongly baked into `src/`)
    // must load purely as `examples/identities/<name>/` DATA packages via the
    // data-driven loader, with ZERO `BuiltinIdentityLoader` registration.

    #[test]
    fn atelier_example_parses_and_loads() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let base = repo_root.join(DEFAULT_EXAMPLE_IDENTITIES_DIR);
        let manifest = load_example_identity(&base, "atelier", &test_request("atelier"))
            .expect("the examples/identities/atelier data package must load");
        assert_eq!(manifest.name, "atelier");
        assert_eq!(
            manifest.default_mode,
            OperatingMode::Engineer,
            "atelier is an agentic example that engineers a fabrication package via external CAD tooling"
        );
        assert_eq!(
            manifest.prompt_assets.len(),
            6,
            "atelier ships system + brief + model + render + fabricate + handoff phase prompts"
        );

        // Data-driven, not compiled in: BuiltinIdentityLoader must NOT know it.
        let builtin_err = BuiltinIdentityLoader
            .load(&test_request("atelier"))
            .expect_err("atelier must NOT be registered in BuiltinIdentityLoader");
        let _ = builtin_err;
    }

    #[test]
    fn concierge_example_parses_and_loads() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let base = repo_root.join(DEFAULT_EXAMPLE_IDENTITIES_DIR);
        let manifest = load_example_identity(&base, "concierge", &test_request("concierge"))
            .expect("the examples/identities/concierge data package must load");
        assert_eq!(manifest.name, "concierge");
        assert_eq!(
            manifest.default_mode,
            OperatingMode::Curator,
            "concierge is an agentic example that designs a hotel concept + operations package"
        );
        assert_eq!(
            manifest.prompt_assets.len(),
            5,
            "concierge ships system + intake + experience + operations + deliver phase prompts"
        );

        // Data-driven, not compiled in: BuiltinIdentityLoader must NOT know it.
        let builtin_err = BuiltinIdentityLoader
            .load(&test_request("concierge"))
            .expect_err("concierge must NOT be registered in BuiltinIdentityLoader");
        let _ = builtin_err;
    }

    /// Simard's OWN operating identities MUST remain compiled into the builtin
    /// loader — deleting the atelier/concierge example arms must not touch them.
    #[test]
    fn own_operating_identities_remain_builtin() {
        for own in [
            "simard-engineer",
            "simard-meeting",
            "simard-gym",
            "simard-goal-curator",
            "simard-improvement-curator",
            "simard-composite-engineer",
        ] {
            BuiltinIdentityLoader
                .load(&test_request(own))
                .unwrap_or_else(|e| {
                    panic!("own identity {own:?} must stay builtin-loadable: {e:?}")
                });
        }
    }

    /// The removed example arms must be GONE from the builtin loader.
    #[test]
    fn removed_example_arms_absent_from_builtin() {
        for removed in ["simard-atelier", "simard-concierge"] {
            let err = BuiltinIdentityLoader
                .load(&test_request(removed))
                .expect_err(&format!(
                    "{removed:?} example arm must be removed from BuiltinIdentityLoader"
                ));
            let _ = err;
        }
    }
}
