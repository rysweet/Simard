//! File-based identity loader that reads identity.toml files.
//!
//! `FileIdentityLoader` wraps `BuiltinIdentityLoader` and adds
//! TOML-based identity loading from a configurable directory.
//! On `load()`: try reading `identity_path/identity.toml`, parse
//! the matching identity name from the `[[identities]]` array.
//! On file-not-found or identity-not-found-in-TOML → delegate to
//! `BuiltinIdentityLoader`. On parse error → return error.

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

use tracing::warn;

use super::loader::BuiltinIdentityLoader;
use super::toml_types::{TomlIdentity, TomlIdentityFile};
use super::{IdentityLoadRequest, IdentityLoader, IdentityManifest, MemoryPolicy, OperatingMode};
use crate::base_types::{BaseTypeCapability, BaseTypeId};
use crate::error::{SimardError, SimardResult};
use crate::memory::MemoryScope;
use crate::prompt_assets::PromptAssetRef;

/// Maximum file size for identity.toml before parsing (1 MB).
const MAX_IDENTITY_FILE_SIZE: u64 = 1_048_576;

/// Maximum depth for recursive composite identity loading.
const MAX_COMPOSITION_DEPTH: usize = 8;

/// Maximum length for identity names (ASCII alphanumeric + hyphens).
const IDENTITY_NAME_MAX_LEN: usize = 128;

/// File-based identity loader with fallback to builtin identities.
pub struct FileIdentityLoader {
    identity_path: PathBuf,
    prompt_root: PathBuf,
    fallback: BuiltinIdentityLoader,
}

impl FileIdentityLoader {
    pub fn new(identity_path: impl Into<PathBuf>, prompt_root: impl Into<PathBuf>) -> Self {
        Self {
            identity_path: identity_path.into(),
            prompt_root: prompt_root.into(),
            fallback: BuiltinIdentityLoader,
        }
    }

    pub fn identity_path(&self) -> &Path {
        &self.identity_path
    }

    fn resolve_identity(
        &self,
        identity: &TomlIdentity,
        file: &TomlIdentityFile,
        request: &IdentityLoadRequest,
        toml_path: &Path,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> SimardResult<IdentityManifest> {
        let default_mode: OperatingMode = identity.default_mode.parse().map_err(|e: String| {
            SimardError::IdentityTomlParseError {
                path: toml_path.to_path_buf(),
                reason: format!("invalid default_mode '{}': {e}", identity.default_mode),
            }
        })?;

        if !identity.components.is_empty() {
            if depth >= MAX_COMPOSITION_DEPTH {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path.to_path_buf(),
                    reason: format!("composition depth exceeds maximum of {MAX_COMPOSITION_DEPTH}"),
                });
            }

            // Track the current DFS path (not a global visited set) so
            // diamond graphs (A→B, A→C, B→D, C→D) are allowed while true
            // cycles (A→B→A) are still detected.
            if !visited.insert(identity.name.clone()) {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path.to_path_buf(),
                    reason: format!(
                        "circular component reference detected for '{}'",
                        identity.name
                    ),
                });
            }

            let mut components = Vec::new();
            for component_name in &identity.components {
                let component = file
                    .identities
                    .iter()
                    .find(|i| &i.name == component_name)
                    .ok_or_else(|| SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!("component identity '{component_name}' not found"),
                    })?;
                components.push(self.resolve_identity(
                    component,
                    file,
                    request,
                    toml_path,
                    visited,
                    depth + 1,
                )?);
            }

            // Remove from path on unwind so sibling branches can revisit
            // the same node (diamond pattern).
            visited.remove(&identity.name);

            return IdentityManifest::compose(
                &identity.name,
                &request.package_version,
                components,
                default_mode,
                request.contract.clone(),
            );
        }

        let supported_base_types = identity
            .supported_base_types
            .iter()
            .map(BaseTypeId::new)
            .collect();

        let required_capabilities = identity
            .required_capabilities
            .iter()
            .map(|s| {
                s.parse::<BaseTypeCapability>()
                    .map_err(|e| SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!("invalid capability '{s}': {e}"),
                    })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;

        let prompt_assets = identity
            .prompt_assets
            .iter()
            .map(|a| {
                validate_prompt_asset_path(&a.path, toml_path)?;
                let identity_dir = toml_path.parent().unwrap_or(toml_path);
                let resolved = identity_dir.join(&a.path);
                if !resolved.is_file() {
                    return Err(SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!(
                            "prompt asset file '{}' not found (resolved to '{}')",
                            a.path,
                            resolved.display()
                        ),
                    });
                }
                // Symlink containment: canonicalize the resolved path and
                // verify it still lives under the identity directory.
                let canonical_resolved =
                    resolved
                        .canonicalize()
                        .map_err(|e| SimardError::IdentityTomlParseError {
                            path: toml_path.to_path_buf(),
                            reason: format!(
                                "cannot canonicalize prompt asset '{}': {e}",
                                resolved.display()
                            ),
                        })?;
                let canonical_identity_dir = identity_dir.canonicalize().map_err(|e| {
                    SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!(
                            "cannot canonicalize identity directory '{}': {e}",
                            identity_dir.display()
                        ),
                    }
                })?;
                if !canonical_resolved.starts_with(&canonical_identity_dir) {
                    return Err(SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!(
                            "prompt asset path '{}' escapes identity directory \
                             (possible symlink attack)",
                            a.path,
                        ),
                    });
                }
                // Store the path relative to prompt_root (not identity dir)
                // so FilePromptAssetStore can resolve it correctly via
                // prompt_root.join(relative_path).
                let canonical_prompt_root = self.prompt_root.canonicalize().map_err(|e| {
                    SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!(
                            "cannot canonicalize prompt root '{}': {e}",
                            self.prompt_root.display()
                        ),
                    }
                })?;
                let relative_to_prompt_root = canonical_resolved
                    .strip_prefix(&canonical_prompt_root)
                    .map_err(|_| SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!(
                            "prompt asset '{}' is not under prompt root '{}'",
                            a.path,
                            self.prompt_root.display()
                        ),
                    })?;
                Ok(PromptAssetRef::new(&a.id, relative_to_prompt_root))
            })
            .collect::<SimardResult<Vec<_>>>()?;

        let memory_policy = match &identity.memory_policy {
            Some(p) => {
                let summary_scope: MemoryScope = p.summary_scope.parse().map_err(|e: String| {
                    SimardError::IdentityTomlParseError {
                        path: toml_path.to_path_buf(),
                        reason: format!("invalid summary_scope '{}': {e}", p.summary_scope),
                    }
                })?;
                MemoryPolicy {
                    allow_project_writes: p.allow_project_writes,
                    summary_scope,
                }
            }
            None => MemoryPolicy::default(),
        };

        IdentityManifest::new(
            &identity.name,
            &request.package_version,
            prompt_assets,
            supported_base_types,
            required_capabilities,
            default_mode,
            memory_policy,
            request.contract.clone(),
        )
    }
}

fn validate_identity_name(name: &str, toml_path: &Path) -> SimardResult<()> {
    if name.is_empty() {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: "identity name cannot be empty".to_string(),
        });
    }
    if name.len() > IDENTITY_NAME_MAX_LEN {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: format!(
                "identity name exceeds maximum length of {IDENTITY_NAME_MAX_LEN} characters"
            ),
        });
    }
    if !name.is_ascii() {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: "identity name must contain only ASCII characters".to_string(),
        });
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: "identity name must contain only ASCII alphanumeric characters and hyphens"
                .to_string(),
        });
    }
    Ok(())
}

fn validate_prompt_asset_path(path_str: &str, toml_path: &Path) -> SimardResult<()> {
    let path = Path::new(path_str);
    if path.is_absolute() {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: format!("absolute path not allowed in prompt asset: '{path_str}'"),
        });
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: format!("path traversal not allowed in prompt asset: '{path_str}'"),
        });
    }
    Ok(())
}

impl IdentityLoader for FileIdentityLoader {
    fn load(&self, request: &IdentityLoadRequest) -> SimardResult<IdentityManifest> {
        let toml_path = self.identity_path.join("identity.toml");

        validate_identity_name(&request.identity, &toml_path)?;

        // Containment check: identity_path must be under prompt_root.
        // If either path doesn't exist yet, we still enforce the check
        // using the raw (non-canonicalized) paths as a best-effort guard.
        match (
            self.identity_path.canonicalize(),
            self.prompt_root.canonicalize(),
        ) {
            (Ok(canon_identity), Ok(canon_root)) => {
                if !canon_identity.starts_with(&canon_root) {
                    return Err(SimardError::IdentityPathNotUnderPromptRoot {
                        identity_path: self.identity_path.clone(),
                        prompt_root: self.prompt_root.clone(),
                    });
                }
            }
            _ => {
                // Canonicalize failed (path doesn't exist yet) — fall back
                // to lexical prefix check on raw paths.
                warn!(
                    identity_path = %self.identity_path.display(),
                    prompt_root = %self.prompt_root.display(),
                    "canonicalize failed for identity or prompt root path; falling back to lexical prefix check"
                );

                // Reject symlinks in identity_path that could escape prompt_root.
                if let Ok(meta) = std::fs::symlink_metadata(&self.identity_path)
                    && meta.is_symlink()
                {
                    // Attempt to resolve the symlink target and check containment.
                    match std::fs::read_link(&self.identity_path) {
                        Ok(target) => {
                            let resolved = if target.is_absolute() {
                                target
                            } else {
                                self.identity_path
                                    .parent()
                                    .unwrap_or(Path::new("."))
                                    .join(&target)
                            };
                            if !resolved.starts_with(&self.prompt_root) {
                                return Err(SimardError::IdentityPathNotUnderPromptRoot {
                                    identity_path: self.identity_path.clone(),
                                    prompt_root: self.prompt_root.clone(),
                                });
                            }
                        }
                        Err(_) => {
                            return Err(SimardError::IdentityPathNotUnderPromptRoot {
                                identity_path: self.identity_path.clone(),
                                prompt_root: self.prompt_root.clone(),
                            });
                        }
                    }
                }

                if !self.identity_path.starts_with(&self.prompt_root) {
                    return Err(SimardError::IdentityPathNotUnderPromptRoot {
                        identity_path: self.identity_path.clone(),
                        prompt_root: self.prompt_root.clone(),
                    });
                }
            }
        }

        // Symlink containment: ensure identity.toml itself does not
        // escape the identity directory via symlink.
        if toml_path.exists() {
            let canonical_toml =
                toml_path
                    .canonicalize()
                    .map_err(|e| SimardError::IdentityTomlParseError {
                        path: toml_path.clone(),
                        reason: format!("cannot canonicalize identity.toml: {e}"),
                    })?;
            let canonical_identity_dir = self.identity_path.canonicalize().map_err(|e| {
                SimardError::IdentityTomlParseError {
                    path: toml_path.clone(),
                    reason: format!(
                        "cannot canonicalize identity directory '{}': {e}",
                        self.identity_path.display()
                    ),
                }
            })?;
            if !canonical_toml.starts_with(&canonical_identity_dir) {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path,
                    reason: "identity.toml escapes identity directory (possible symlink attack)"
                        .to_string(),
                });
            }
        }

        // Check file size via metadata BEFORE reading to avoid loading
        // an oversized file into memory.
        match std::fs::metadata(&toml_path) {
            Ok(meta) => {
                if meta.len() > MAX_IDENTITY_FILE_SIZE {
                    return Err(SimardError::IdentityTomlParseError {
                        path: toml_path,
                        reason: format!(
                            "file exceeds maximum size of {MAX_IDENTITY_FILE_SIZE} bytes"
                        ),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.fallback.load(request);
            }
            Err(e) => {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path,
                    reason: e.to_string(),
                });
            }
        }

        let bytes = match std::fs::read(&toml_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.fallback.load(request);
            }
            Err(e) => {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path,
                    reason: e.to_string(),
                });
            }
        };

        let content =
            String::from_utf8(bytes).map_err(|e| SimardError::IdentityTomlParseError {
                path: toml_path.clone(),
                reason: format!("file is not valid UTF-8: {e}"),
            })?;

        let file: TomlIdentityFile =
            toml::from_str(&content).map_err(|e| SimardError::IdentityTomlParseError {
                path: toml_path.clone(),
                reason: e.to_string(),
            })?;

        let identity = match file.identities.iter().find(|i| i.name == request.identity) {
            Some(i) => i,
            None => return self.fallback.load(request),
        };

        let mut visited = HashSet::new();
        self.resolve_identity(identity, &file, request, &toml_path, &mut visited, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ManifestContract, OperatingMode};
    use crate::metadata::{Freshness, Provenance};
    use std::fs;
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

    fn write_identity_toml(dir: &Path, content: &str) {
        fs::write(dir.join("identity.toml"), content).unwrap();
    }

    const ENGINEER_TOML: &str = r#"
[package]
name = "test-identity"
version = "0.1.0"

[[identities]]
name = "test-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory", "evidence", "reflection"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "engineer_system.md"
"#;

    // ── Happy path ──────────────────────────────────────────────────

    #[test]
    fn file_loader_loads_identity_from_toml() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("custom");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, ENGINEER_TOML);
        fs::write(identity_dir.join("engineer_system.md"), "# System prompt").unwrap();

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("test-engineer")).unwrap();
        assert_eq!(manifest.name, "test-engineer");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
        assert!(!manifest.prompt_assets.is_empty());
    }

    // ── Fallback behavior ───────────────────────────────────────────

    #[test]
    fn file_loader_falls_back_when_no_toml_file() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("empty-identity");
        fs::create_dir_all(&identity_dir).unwrap();

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("simard-engineer")).unwrap();
        assert_eq!(manifest.name, "simard-engineer");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    }

    #[test]
    fn file_loader_falls_back_when_identity_not_in_toml() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("partial");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, ENGINEER_TOML);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        // "simard-engineer" is not in this TOML → builtin fallback
        let manifest = loader.load(&test_request("simard-engineer")).unwrap();
        assert_eq!(manifest.name, "simard-engineer");
    }

    // ── Error cases ─────────────────────────────────────────────────

    #[test]
    fn file_loader_hard_error_on_malformed_toml() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("bad");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, "this is not valid TOML {{{}}}");

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("anything")).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "malformed TOML should produce IdentityTomlParseError, got: {err:?}"
        );
    }

    #[test]
    fn file_loader_rejects_oversized_file() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("large");
        fs::create_dir_all(&identity_dir).unwrap();
        let large_content = "x".repeat(MAX_IDENTITY_FILE_SIZE as usize + 1);
        write_identity_toml(&identity_dir, &large_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("anything")).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "oversized file should be rejected, got: {err:?}"
        );
    }

    // ── Identity name validation ────────────────────────────────────

    #[test]
    fn file_loader_validates_identity_name_ascii_only() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("unicode");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "identité-café"
default_mode = "engineer"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("identité-café")).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "non-ASCII identity name should be rejected, got: {err:?}"
        );
    }

    #[test]
    fn file_loader_validates_identity_name_max_length() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("long-name");
        fs::create_dir_all(&identity_dir).unwrap();
        let long_name = "a".repeat(IDENTITY_NAME_MAX_LEN + 1);
        let toml_content = format!(
            r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "{long_name}"
default_mode = "engineer"
"#
        );
        write_identity_toml(&identity_dir, &toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request(&long_name)).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "overly long identity name should be rejected, got: {err:?}"
        );
    }

    #[test]
    fn file_loader_validates_identity_name_nonempty() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("empty-name");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = ""
default_mode = "engineer"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("")).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "empty identity name should be rejected, got: {err:?}"
        );
    }

    #[test]
    fn file_loader_validates_identity_name_alphanumeric_hyphens() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("bad-chars");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "identity with spaces!"
default_mode = "engineer"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader
            .load(&test_request("identity with spaces!"))
            .unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "identity name with spaces/special chars should be rejected, got: {err:?}"
        );
    }

    // ── Path validation ─────────────────────────────────────────────

    #[test]
    fn file_loader_rejects_absolute_prompt_asset_path() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("abs-path");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test-abs"
default_mode = "engineer"

[[identities.prompt_assets]]
id = "system"
path = "/etc/passwd"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("test-abs")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("absolute") || msg.contains("traversal") || msg.contains("path"),
            "absolute path should be rejected, got: {msg}"
        );
    }

    #[test]
    fn file_loader_rejects_path_traversal_in_prompt_asset() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("traversal");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test-trav"
default_mode = "engineer"

[[identities.prompt_assets]]
id = "system"
path = "../../etc/passwd"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("test-trav")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("traversal") || msg.contains("not allowed") || msg.contains("path"),
            "path traversal should be rejected, got: {msg}"
        );
    }

    #[test]
    fn file_loader_rejects_identity_path_outside_prompt_root() {
        let prompt_root = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();
        let identity_dir = other_dir.path().join("rogue");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, ENGINEER_TOML);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("test-engineer")).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityPathNotUnderPromptRoot { .. }),
            "identity path outside prompt root should be rejected, got: {err:?}"
        );
    }

    // ── Composite identities ────────────────────────────────────────

    #[test]
    fn file_loader_detects_circular_component_references() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("circular");
        fs::create_dir_all(&identity_dir).unwrap();
        let toml_content = r#"
[package]
name = "circular"
version = "0.1.0"

[[identities]]
name = "loop-a"
default_mode = "engineer"
components = ["loop-b"]

[[identities]]
name = "loop-b"
default_mode = "engineer"
components = ["loop-a"]
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("loop-a")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("circular") || msg.contains("cycle") || msg.contains("depth"),
            "circular reference should be detected, got: {msg}"
        );
    }

    #[test]
    fn file_loader_enforces_composition_depth_limit() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("deep");
        fs::create_dir_all(&identity_dir).unwrap();
        // Chain deeper than MAX_COMPOSITION_DEPTH
        let mut toml_content = String::from(
            r#"
[package]
name = "deep"
version = "0.1.0"
"#,
        );
        for i in 0..=MAX_COMPOSITION_DEPTH {
            let name = format!("level-{i}");
            let component = format!("level-{}", i + 1);
            toml_content.push_str(&format!(
                r#"
[[identities]]
name = "{name}"
default_mode = "engineer"
components = ["{component}"]
"#,
            ));
        }
        let leaf_name = format!("level-{}", MAX_COMPOSITION_DEPTH + 1);
        toml_content.push_str(&format!(
            r#"
[[identities]]
name = "{leaf_name}"
default_mode = "engineer"
supported_base_types = ["local-harness"]
"#,
        ));
        write_identity_toml(&identity_dir, &toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("level-0")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("depth") || msg.contains("too deep") || msg.contains("recursive"),
            "depth limit should be enforced, got: {msg}"
        );
    }

    #[test]
    fn file_loader_composite_loads_components() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("composite");
        fs::create_dir_all(&identity_dir).unwrap();
        fs::write(identity_dir.join("a_system.md"), "# A system").unwrap();
        fs::write(identity_dir.join("b_system.md"), "# B system").unwrap();
        let toml_content = r#"
[package]
name = "composite"
version = "0.1.0"

[[identities]]
name = "comp-a"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets"]

[[identities.prompt_assets]]
id = "a-system"
path = "a_system.md"

[[identities]]
name = "comp-b"
default_mode = "meeting"
supported_base_types = ["local-harness"]
required_capabilities = ["memory"]

[[identities.prompt_assets]]
id = "b-system"
path = "b_system.md"

[[identities]]
name = "composite-all"
default_mode = "engineer"
components = ["comp-a", "comp-b"]
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("composite-all")).unwrap();
        assert_eq!(manifest.name, "composite-all");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
        assert!(!manifest.components.is_empty());
        assert!(
            manifest.prompt_assets.len() >= 2,
            "composite should merge prompt assets from components"
        );
    }

    // ── Accessor ────────────────────────────────────────────────────

    #[test]
    fn file_loader_exposes_identity_path() {
        let path = PathBuf::from("/some/identity/path");
        let loader = FileIdentityLoader::new(&path, "/some/root");
        assert_eq!(loader.identity_path(), path);
    }

    // ── Finding 7: nonexistent identity.toml → fallback ─────────────

    #[test]
    fn file_loader_falls_back_for_nonexistent_identity_path() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("does-not-exist");
        // identity_dir is never created — identity.toml will be NotFound.

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("simard-engineer")).unwrap();
        assert_eq!(manifest.name, "simard-engineer");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    }

    // ── Finding 5: prompt asset path resolution & existence ─────────

    #[test]
    fn file_loader_rejects_missing_prompt_asset_file() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("missing-asset");
        fs::create_dir_all(&identity_dir).unwrap();
        // Write TOML referencing a prompt asset file that does NOT exist.
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test-missing"
default_mode = "engineer"
supported_base_types = ["local-harness"]

[[identities.prompt_assets]]
id = "system"
path = "does_not_exist.md"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader.load(&test_request("test-missing")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found") || msg.contains("does_not_exist.md"),
            "missing prompt asset file should be rejected, got: {msg}"
        );
    }

    #[test]
    fn file_loader_resolves_prompt_asset_relative_to_identity_dir() {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("relative-test");
        let sub_dir = identity_dir.join("prompts");
        fs::create_dir_all(&sub_dir).unwrap();
        // Create the prompt asset in a subdirectory.
        fs::write(sub_dir.join("system.md"), "# System prompt").unwrap();
        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test-relative"
default_mode = "engineer"
supported_base_types = ["local-harness"]

[[identities.prompt_assets]]
id = "system"
path = "prompts/system.md"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("test-relative")).unwrap();
        assert_eq!(manifest.prompt_assets.len(), 1);
        assert_eq!(manifest.prompt_assets[0].id.as_str(), "system");
    }

    #[cfg(unix)]
    #[test]
    fn file_loader_rejects_symlink_escaping_identity_dir() {
        use std::os::unix::fs as unix_fs;

        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("symlink-escape");
        let prompts_dir = identity_dir.join("prompts");
        fs::create_dir_all(&prompts_dir).unwrap();

        // Create a file outside the identity directory to be the symlink target.
        let outside_file = prompt_root.path().join("secret.txt");
        fs::write(&outside_file, "secret content").unwrap();

        // Create a symlink inside prompts/ that points outside the identity dir.
        unix_fs::symlink(&outside_file, prompts_dir.join("evil.md")).unwrap();

        let toml_content = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test-symlink"
default_mode = "engineer"
supported_base_types = ["local-harness"]

[[identities.prompt_assets]]
id = "evil"
path = "prompts/evil.md"
"#;
        write_identity_toml(&identity_dir, toml_content);

        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let err = loader
            .load(&test_request("test-symlink"))
            .expect_err("symlink escaping identity dir should be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("escapes identity directory"),
            "error should mention directory escape, got: {msg}"
        );
    }
}
