//! TOML deserialization types for identity.toml files.
//!
//! These types are deliberately separate from the core identity types
//! (`IdentityManifest`, `OperatingMode`, etc.) to keep TOML serialization
//! concerns out of the domain model. Used by `FileIdentityLoader`.

use serde::Deserialize;

/// Top-level structure of an identity.toml file.
#[derive(Debug, Deserialize)]
pub(crate) struct TomlIdentityFile {
    #[allow(dead_code)]
    pub package: TomlPackage,
    #[serde(default)]
    pub identities: Vec<TomlIdentity>,
}

/// The `[package]` table in identity.toml.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct TomlPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A single `[[identities]]` entry in identity.toml.
#[derive(Debug, Deserialize)]
pub(crate) struct TomlIdentity {
    pub name: String,
    pub default_mode: String,
    #[serde(default)]
    pub supported_base_types: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub prompt_assets: Vec<TomlPromptAsset>,
    #[serde(default)]
    pub components: Vec<String>,
    #[serde(default)]
    pub memory_policy: Option<TomlMemoryPolicy>,
}

/// A `[[identities.prompt_assets]]` entry.
#[derive(Debug, Deserialize)]
pub(crate) struct TomlPromptAsset {
    pub id: String,
    pub path: String,
}

/// An optional `[identities.memory_policy]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TomlMemoryPolicy {
    #[serde(default)]
    pub allow_project_writes: bool,
    #[serde(default = "default_summary_scope")]
    pub summary_scope: String,
}

fn default_summary_scope() -> String {
    "session-summary".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
[package]
name = "test-identity"
version = "0.1.0"

[[identities]]
name = "test-engineer"
default_mode = "engineer"
"#;

    const FULL_TOML: &str = r#"
[package]
name = "test-identity"
version = "0.1.0"
description = "Test identity package"

[[identities]]
name = "test-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["prompt-assets", "memory"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "engineer_system.md"

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"
"#;

    // --- Happy path ---

    #[test]
    fn parse_minimal_identity_file() {
        let file: TomlIdentityFile = toml::from_str(MINIMAL_TOML).unwrap();
        assert_eq!(file.package.name, "test-identity");
        assert_eq!(file.package.version, "0.1.0");
        assert!(file.package.description.is_none());
        assert_eq!(file.identities.len(), 1);
        assert_eq!(file.identities[0].name, "test-engineer");
        assert_eq!(file.identities[0].default_mode, "engineer");
    }

    #[test]
    fn parse_full_identity_file() {
        let file: TomlIdentityFile = toml::from_str(FULL_TOML).unwrap();
        assert_eq!(
            file.package.description.as_deref(),
            Some("Test identity package")
        );
        let identity = &file.identities[0];
        assert_eq!(
            identity.supported_base_types,
            vec!["local-harness", "rusty-clawd"]
        );
        assert_eq!(
            identity.required_capabilities,
            vec!["prompt-assets", "memory"]
        );
        assert_eq!(identity.prompt_assets.len(), 1);
        assert_eq!(identity.prompt_assets[0].id, "engineer-system");
        assert_eq!(identity.prompt_assets[0].path, "engineer_system.md");
        let policy = identity.memory_policy.as_ref().unwrap();
        assert!(!policy.allow_project_writes);
        assert_eq!(policy.summary_scope, "session-summary");
    }

    // --- deny_unknown_fields ---

    #[test]
    fn parse_rejects_unknown_package_field() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"
unknown_field = "bad"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [package] should be rejected"
        );
    }

    #[test]
    fn parse_accepts_unknown_identity_field() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test"
default_mode = "engineer"
flavor = "vanilla"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(
            result.is_ok(),
            "unknown field in [[identities]] should be accepted for forward compatibility"
        );
    }

    #[test]
    fn parse_accepts_unknown_prompt_asset_field() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test"
default_mode = "engineer"

[[identities.prompt_assets]]
id = "sys"
path = "system.md"
weight = 5
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(
            result.is_ok(),
            "unknown field in prompt_assets should be accepted for forward compatibility"
        );
    }

    #[test]
    fn parse_rejects_unknown_memory_policy_field() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test"
default_mode = "engineer"

[identities.memory_policy]
allow_project_writes = false
encryption = "aes"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(
            result.is_err(),
            "unknown field in memory_policy should be rejected"
        );
    }

    // --- Missing required fields ---

    #[test]
    fn parse_rejects_missing_package_name() {
        let toml_str = r#"
[package]
version = "0.1.0"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_missing_package_version() {
        let toml_str = r#"
[package]
name = "test"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_missing_identity_name() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
default_mode = "engineer"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_missing_identity_default_mode() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test"
"#;
        let result: Result<TomlIdentityFile, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    // --- Composite identities ---

    #[test]
    fn parse_identity_with_components() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "composite"
default_mode = "engineer"
components = ["child-a", "child-b"]
"#;
        let file: TomlIdentityFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.identities[0].components, vec!["child-a", "child-b"]);
    }

    // --- Multiple identities ---

    #[test]
    fn parse_multiple_identities() {
        let toml_str = r#"
[package]
name = "multi"
version = "0.1.0"

[[identities]]
name = "mode-a"
default_mode = "engineer"

[[identities]]
name = "mode-b"
default_mode = "meeting"
"#;
        let file: TomlIdentityFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.identities.len(), 2);
        assert_eq!(file.identities[0].name, "mode-a");
        assert_eq!(file.identities[1].name, "mode-b");
    }

    // --- Edge cases ---

    #[test]
    fn parse_memory_policy_defaults() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "test"
default_mode = "engineer"

[identities.memory_policy]
"#;
        let file: TomlIdentityFile = toml::from_str(toml_str).unwrap();
        let policy = file.identities[0].memory_policy.as_ref().unwrap();
        assert!(!policy.allow_project_writes);
        assert_eq!(policy.summary_scope, "session-summary");
    }

    #[test]
    fn parse_identity_with_empty_optional_fields() {
        let file: TomlIdentityFile = toml::from_str(MINIMAL_TOML).unwrap();
        assert!(file.identities[0].prompt_assets.is_empty());
        assert!(file.identities[0].components.is_empty());
        assert!(file.identities[0].supported_base_types.is_empty());
        assert!(file.identities[0].required_capabilities.is_empty());
        assert!(file.identities[0].memory_policy.is_none());
    }

    #[test]
    fn parse_file_with_no_identities() {
        let toml_str = r#"
[package]
name = "empty"
version = "0.1.0"
"#;
        let file: TomlIdentityFile = toml::from_str(toml_str).unwrap();
        assert!(file.identities.is_empty());
    }

    #[test]
    fn parse_identity_with_multiple_prompt_assets() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[[identities]]
name = "multi-prompt"
default_mode = "engineer"

[[identities.prompt_assets]]
id = "system-a"
path = "a.md"

[[identities.prompt_assets]]
id = "system-b"
path = "b.md"
"#;
        let file: TomlIdentityFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.identities[0].prompt_assets.len(), 2);
        assert_eq!(file.identities[0].prompt_assets[0].id, "system-a");
        assert_eq!(file.identities[0].prompt_assets[1].id, "system-b");
    }
}
