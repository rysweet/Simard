use std::collections::BTreeSet;

use crate::base_types::{BaseTypeCapability, BaseTypeId};
use crate::error::{SimardError, SimardResult};
use crate::prompt_assets::PromptAssetRef;

use super::{ManifestContract, MemoryPolicy, OperatingMode};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityManifest {
    pub name: String,
    pub version: String,
    pub prompt_assets: Vec<PromptAssetRef>,
    pub components: Vec<String>,
    pub supported_base_types: Vec<BaseTypeId>,
    pub required_capabilities: BTreeSet<BaseTypeCapability>,
    pub default_mode: OperatingMode,
    pub memory_policy: MemoryPolicy,
    pub contract: ManifestContract,
}

impl IdentityManifest {
    #[expect(
        clippy::too_many_arguments,
        reason = "identity manifests are explicit contract values with distinct fields"
    )]
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        prompt_assets: Vec<PromptAssetRef>,
        supported_base_types: Vec<BaseTypeId>,
        required_capabilities: BTreeSet<BaseTypeCapability>,
        default_mode: OperatingMode,
        memory_policy: MemoryPolicy,
        contract: ManifestContract,
    ) -> SimardResult<Self> {
        memory_policy.validate()?;

        Ok(Self {
            name: name.into(),
            version: version.into(),
            prompt_assets,
            components: Vec::new(),
            supported_base_types,
            required_capabilities,
            default_mode,
            memory_policy,
            contract,
        })
    }

    pub fn with_components(
        mut self,
        components: impl IntoIterator<Item = impl Into<String>>,
    ) -> SimardResult<Self> {
        let mut seen = BTreeSet::new();
        let mut normalized = Vec::new();
        for component in components {
            let component = component.into().trim().to_string();
            if component.is_empty() {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: "component identities cannot be empty".to_string(),
                });
            }
            if component == self.name {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: "an identity cannot list itself as a component".to_string(),
                });
            }
            if !seen.insert(component.clone()) {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: format!("duplicate component identity '{component}'"),
                });
            }
            normalized.push(component);
        }
        self.components = normalized;
        Ok(self)
    }

    pub fn supports_base_type(&self, base_type: &BaseTypeId) -> bool {
        self.supported_base_types
            .iter()
            .any(|candidate| candidate == base_type)
    }
}

/// Compose multiple identity manifests using precedence-based conflict
/// resolution. Index 0 in the input `Vec` is the highest-precedence manifest.
///
/// Delegates to [`crate::identity_precedence::PrecedenceResolver`].
pub fn compose_with_precedence(
    manifests: Vec<IdentityManifest>,
) -> crate::identity_precedence::ResolvedIdentity {
    crate::identity_precedence::PrecedenceResolver::new(manifests).resolve_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::capability_set;
    use crate::metadata::{Freshness, Provenance};

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
    fn identity_manifest_supports_base_type_check() {
        let manifest = IdentityManifest::new(
            "test-identity",
            "0.1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
        assert!(!manifest.supports_base_type(&BaseTypeId::new("unknown")));
    }

    // --- IdentityManifest ---

    #[test]
    fn identity_manifest_new_rejects_project_writes_policy() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_memory_type: crate::memory::CognitiveMemoryType::Episodic,
        };
        let err = IdentityManifest::new(
            "test",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            policy,
            test_contract(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::UnsupportedMemoryPolicy { .. }));
    }

    #[test]
    fn identity_manifest_with_components_success() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
        .with_components(["child-a", "child-b"])
        .unwrap();
        assert_eq!(manifest.components, vec!["child-a", "child-b"]);
    }

    #[test]
    fn identity_manifest_with_components_rejects_empty() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest.with_components(["  "]).unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn identity_manifest_with_components_rejects_self_reference() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest.with_components(["parent"]).unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn identity_manifest_with_components_rejects_duplicates() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest
            .with_components(["child-a", "child-a"])
            .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn supports_base_type_returns_false_for_nonexistent() {
        let manifest = IdentityManifest::new(
            "test",
            "1.0",
            vec![],
            vec![
                BaseTypeId::new("local-harness"),
                BaseTypeId::new("rusty-clawd"),
            ],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
        assert!(manifest.supports_base_type(&BaseTypeId::new("rusty-clawd")));
        assert!(!manifest.supports_base_type(&BaseTypeId::new("nonexistent")));
    }
}
