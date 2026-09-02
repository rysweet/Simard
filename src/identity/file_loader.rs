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
use super::toml_types::{TomlAuthority, TomlIdentity, TomlIdentityFile};
use super::{
    IdentityAuthority, IdentityLoadRequest, IdentityLoader, IdentityManifest, MemoryPolicy,
    OperatingMode, SeedGoal, WritePosture,
};
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

        // #3125: identity-scoped cognition fields (seed goals, target scope,
        // write-authority posture). Computed once and applied to whichever
        // manifest is built (leaf or composite).
        let seed_goals: Vec<SeedGoal> = identity
            .seed_goals
            .iter()
            .map(|g| {
                let seed = SeedGoal::new(
                    g.priority,
                    g.title.clone(),
                    g.description.clone(),
                    g.repo.clone(),
                );
                // #4927 three-state mapping: preserve the omitted/explicit
                // distinction the `Option<bool>` carries. Omitted (`None`) is an
                // inert non-standing seed; explicit `false` maps to
                // `.non_standing()` (authorizes conservative reversal); `true`
                // maps to `.standing()`.
                match g.standing {
                    Some(true) => seed.standing(),
                    Some(false) => seed.non_standing(),
                    None => seed,
                }
            })
            .collect();
        let target_repos = identity.target_repos.clone();
        let declared_authority: Option<IdentityAuthority> = match &identity.authority {
            Some(a) => Some(toml_authority_to_domain(a, &identity.name, toml_path)?),
            None => None,
        };

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

            let composed = IdentityManifest::compose(
                &identity.name,
                &request.package_version,
                components,
                default_mode,
                request.contract.clone(),
            )?;
            // A composite may not DILUTE its components' posture: if it declares
            // its own `[identities.authority]`, it must agree with the posture
            // `compose` derived from the components (defense in depth, #3125).
            if let Some(declared) = &declared_authority
                && declared.posture != composed.authority.posture
            {
                return Err(SimardError::IdentityTomlParseError {
                    path: toml_path.to_path_buf(),
                    reason: format!(
                        "composite identity '{}' declares authority.posture = \"{}\" but its \
                         components resolve to \"{}\"; a composite cannot dilute its components' \
                         write-authority posture",
                        identity.name, declared.posture, composed.authority.posture
                    ),
                });
            }
            return Ok(composed
                .with_seed_goals(seed_goals)
                .with_target_repos(target_repos));
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
        .map(|m| {
            m.with_seed_goals(seed_goals)
                .with_target_repos(target_repos)
                .with_authority(declared_authority.unwrap_or_default())
        })
    }
}

/// Convert a `[identities.authority]` TOML block into a validated
/// [`IdentityAuthority`] (#3125 / #3067), failing closed on contradictions.
///
/// Rules (see docs/reference/write-authority-posture-api.md):
/// - `posture` must be one of `read-only | scoped-write | full`.
/// - Under `read-only`, any `allow_*_writes = true` is a hard contradiction.
/// - `allowed_write_repos` must be empty unless `posture = "scoped-write"`.
/// - `allow_*` default is posture-dependent: `false` under `read-only`,
///   `true` otherwise.
fn toml_authority_to_domain(
    authority: &TomlAuthority,
    identity_name: &str,
    toml_path: &Path,
) -> SimardResult<IdentityAuthority> {
    let posture: WritePosture =
        authority
            .posture
            .parse()
            .map_err(|e: String| SimardError::IdentityTomlParseError {
                path: toml_path.to_path_buf(),
                reason: format!(
                    "invalid authority.posture '{}' for identity '{identity_name}': {e}",
                    authority.posture
                ),
            })?;
    let read_only = posture == WritePosture::ReadOnly;

    for (field, value) in [
        ("allow_git_push", authority.allow_git_push),
        ("allow_ado_writes", authority.allow_ado_writes),
        ("allow_github_writes", authority.allow_github_writes),
    ] {
        if read_only && value == Some(true) {
            return Err(SimardError::IdentityTomlParseError {
                path: toml_path.to_path_buf(),
                reason: format!(
                    "authority.{field} = true contradicts posture = \"read-only\" for identity \
                     '{identity_name}' (a read-only identity may not enable any write path)"
                ),
            });
        }
    }

    if !authority.allowed_write_repos.is_empty() && posture != WritePosture::ScopedWrite {
        return Err(SimardError::IdentityTomlParseError {
            path: toml_path.to_path_buf(),
            reason: format!(
                "authority.allowed_write_repos is non-empty but posture is \"{posture}\" for \
                 identity '{identity_name}' (only scoped-write may list writable repos)"
            ),
        });
    }

    let default_allow = !read_only;
    Ok(IdentityAuthority {
        posture,
        allowed_write_repos: authority.allowed_write_repos.clone(),
        allow_git_push: authority.allow_git_push.unwrap_or(default_allow),
        allow_ado_writes: authority.allow_ado_writes.unwrap_or(default_allow),
        allow_github_writes: authority.allow_github_writes.unwrap_or(default_allow),
    })
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

    // ── #3125: identity-scoped cognition (seed goals, target scope, posture) ──

    const CROCUTUS_TOML: &str = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"
target_repos = ["hyenas"]

[[identities.seed_goals]]
priority = 1
title = "Observe hyenas repo health"
description = "Read the hyenas repos and assess branch hygiene, CODEOWNERS, LICENSE, dependabot, large blobs. OBSERVE ONLY."
repo = "hyenas"

[[identities.seed_goals]]
priority = 2
title = "Articulate repo-hygiene backlog"
description = "Turn observations into prioritized, target-scoped repo-hygiene goals on this identity's own board."
repo = "hyenas"

[identities.authority]
posture = "read-only"
"#;

    fn load_crocutus(toml: &str) -> SimardResult<IdentityManifest> {
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("crocutus");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, toml);
        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        loader.load(&test_request("crocutus"))
    }

    #[test]
    fn file_loader_reads_read_only_identity_seed_goals_and_target_scope() {
        let manifest = load_crocutus(CROCUTUS_TOML).expect("crocutus identity must load");
        // (b) A read-only identity seeds its OWN goals, scoped to its target.
        assert_eq!(manifest.seed_goals.len(), 2);
        assert_eq!(manifest.seed_goals[0].priority, 1);
        assert_eq!(manifest.seed_goals[0].title, "Observe hyenas repo health");
        assert_eq!(manifest.seed_goals[0].repo.as_deref(), Some("hyenas"));
        assert_eq!(manifest.target_repos, vec!["hyenas".to_string()]);
        assert_eq!(manifest.resolved_target_repos(), vec!["hyenas".to_string()]);
        // Read-only posture with every write path denied (defaults under read-only).
        assert_eq!(manifest.authority.posture, WritePosture::ReadOnly);
        assert!(!manifest.authority.allow_git_push);
        assert!(!manifest.authority.allow_ado_writes);
        assert!(!manifest.authority.allow_github_writes);
        assert!(!manifest.authority.permits_spawn());
    }

    #[test]
    fn file_loader_preserves_standing_declaration_three_state() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[[identities.seed_goals]]
priority = 1
title = "Explicit standing"
description = "d"
standing = true

[[identities.seed_goals]]
priority = 2
title = "Explicit non-standing"
description = "d"
standing = false

[[identities.seed_goals]]
priority = 3
title = "Omitted standing"
description = "d"
"#;
        let manifest = load_crocutus(toml).expect("three-state identity must load");
        assert_eq!(manifest.seed_goals.len(), 3);

        let explicit_standing = &manifest.seed_goals[0];
        assert!(explicit_standing.standing);
        assert!(!explicit_standing.authorizes_standing_reversal());

        let explicit_non_standing = &manifest.seed_goals[1];
        assert!(!explicit_non_standing.standing);
        assert!(explicit_non_standing.authorizes_standing_reversal());

        let omitted = &manifest.seed_goals[2];
        assert!(!omitted.standing);
        assert!(!omitted.authorizes_standing_reversal());
    }

    #[test]
    fn file_loader_target_repos_defaults_to_union_of_seed_goal_repos() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[[identities.seed_goals]]
priority = 1
title = "Observe hyenas repo health"
description = "OBSERVE ONLY"
repo = "hyenas"

[identities.authority]
posture = "read-only"
"#;
        let manifest = load_crocutus(toml).expect("identity must load");
        // target_repos omitted => union of seed-goal repos.
        assert!(manifest.target_repos.is_empty());
        assert_eq!(manifest.resolved_target_repos(), vec!["hyenas".to_string()]);
    }

    #[test]
    fn file_loader_identity_without_authority_is_full_and_unchanged() {
        // (a) An identity that omits [identities.authority] and declares no seed
        // goals resolves to `full` with no override — Simard-equivalent.
        let toml = r#"
[package]
name = "plain"
version = "0.1.0"

[[identities]]
name = "plain"
default_mode = "engineer"
supported_base_types = ["local-harness"]
"#;
        let prompt_root = TempDir::new().unwrap();
        let identity_dir = prompt_root.path().join("plain");
        fs::create_dir_all(&identity_dir).unwrap();
        write_identity_toml(&identity_dir, toml);
        let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
        let manifest = loader.load(&test_request("plain")).unwrap();
        assert!(manifest.seed_goals.is_empty());
        assert!(manifest.target_repos.is_empty());
        assert_eq!(manifest.authority, IdentityAuthority::default());
        assert!(manifest.authority.permits_spawn());
    }

    #[test]
    fn file_loader_rejects_read_only_write_contradiction() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[identities.authority]
posture = "read-only"
allow_git_push = true
"#;
        let err =
            load_crocutus(toml).expect_err("read-only + allow_git_push=true must be rejected");
        let msg = err.to_string();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "contradiction must be an IdentityTomlParseError, got: {err:?}"
        );
        assert!(
            msg.contains("contradicts posture") && msg.contains("read-only"),
            "error should explain the contradiction, got: {msg}"
        );
    }

    #[test]
    fn file_loader_rejects_allowlist_without_scoped_write() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[identities.authority]
posture = "full"
allowed_write_repos = ["some-repo"]
"#;
        let err = load_crocutus(toml).expect_err("allowed_write_repos under full must be rejected");
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "must be an IdentityTomlParseError, got: {err:?}"
        );
    }

    #[test]
    fn file_loader_rejects_invalid_posture_value() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[identities.authority]
posture = "read-write"
"#;
        let err = load_crocutus(toml).expect_err("unknown posture must be rejected");
        assert!(matches!(err, SimardError::IdentityTomlParseError { .. }));
    }

    #[test]
    fn file_loader_rejects_unknown_seed_goal_field() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[[identities.seed_goals]]
priority = 1
title = "g"
description = "d"
bogus = "x"
"#;
        let err = load_crocutus(toml).expect_err("unknown seed_goal field must be rejected");
        assert!(matches!(err, SimardError::IdentityTomlParseError { .. }));
    }

    #[test]
    fn file_loader_rejects_unknown_authority_field() {
        let toml = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus"
default_mode = "engineer"

[identities.authority]
posture = "read-only"
encryption = "aes"
"#;
        let err = load_crocutus(toml).expect_err("unknown authority field must be rejected");
        assert!(matches!(err, SimardError::IdentityTomlParseError { .. }));
    }
}
