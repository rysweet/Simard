use std::collections::BTreeSet;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::prompt_assets::PromptAssetRef;

use super::{IdentityManifest, ManifestContract, OperatingMode};

impl IdentityManifest {
    pub fn compose(
        name: impl Into<String>,
        version: impl Into<String>,
        components: Vec<IdentityManifest>,
        default_mode: OperatingMode,
        contract: ManifestContract,
    ) -> SimardResult<Self> {
        let name = name.into();
        let version = version.into();
        if components.is_empty() {
            return Err(SimardError::InvalidIdentityComposition {
                identity: name,
                reason: "at least one component identity is required".to_string(),
            });
        }
        let component_names = components
            .iter()
            .map(|component| component.name.clone())
            .collect::<Vec<_>>();

        let mut prompt_assets: Vec<PromptAssetRef> = Vec::new();
        let mut seen_prompt_assets = BTreeSet::new();
        for component in &components {
            for asset in &component.prompt_assets {
                let key = format!("{}::{}", asset.id, asset.relative_path.display());
                if seen_prompt_assets.insert(key) {
                    prompt_assets.push(asset.clone());
                }
            }
        }

        let mut supported_base_types: Vec<BaseTypeId> =
            components[0].supported_base_types.clone();
        supported_base_types.retain(|candidate| {
            components
                .iter()
                .all(|component| component.supported_base_types.contains(candidate))
        });
        if supported_base_types.is_empty() {
            return Err(SimardError::InvalidIdentityComposition {
                identity: name.clone(),
                reason: "component identities do not share a common supported base type"
                    .to_string(),
            });
        }

        let mut required_capabilities = BTreeSet::new();
        for component in &components {
            required_capabilities.extend(component.required_capabilities.iter().copied());
        }

        let memory_policy = components[0].memory_policy.clone();
        if components
            .iter()
            .any(|component| component.memory_policy != memory_policy)
        {
            return Err(SimardError::InvalidIdentityComposition {
                identity: name.clone(),
                reason: "component identities must agree on memory policy".to_string(),
            });
        }

        IdentityManifest::new(
            name,
            version,
            prompt_assets,
            supported_base_types,
            required_capabilities,
            default_mode,
            memory_policy,
            contract,
        )?
        .with_components(component_names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::{BaseTypeCapability, capability_set};
    use crate::identity::MemoryPolicy;
    use crate::memory::MemoryScope;
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
    fn identity_manifest_compose_requires_at_least_one_component() {
        let err = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn identity_manifest_compose_rejects_incompatible_base_types() {
        let m1 = IdentityManifest::new(
            "comp-a",
            "1.0",
            vec![],
            vec![BaseTypeId::new("type-a")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let m2 = IdentityManifest::new(
            "comp-b",
            "1.0",
            vec![],
            vec![BaseTypeId::new("type-b")],
            capability_set([]),
            OperatingMode::Meeting,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![m1, m2],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("common supported base type"));
    }

    #[test]
    fn identity_manifest_compose_rejects_mismatched_memory_policies() {
        let m1 = IdentityManifest::new(
            "comp-a",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let mut m2 = IdentityManifest::new(
            "comp-b",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Meeting,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        m2.memory_policy.summary_scope = MemoryScope::Decision;
        let err = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![m1, m2],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("memory policy"));
    }

    #[test]
    fn identity_manifest_compose_merges_capabilities() {
        let m1 = IdentityManifest::new(
            "comp-a",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([BaseTypeCapability::PromptAssets]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let m2 = IdentityManifest::new(
            "comp-b",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([BaseTypeCapability::Memory]),
            OperatingMode::Meeting,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let composed = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![m1, m2],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap();
        assert!(
            composed
                .required_capabilities
                .contains(&BaseTypeCapability::PromptAssets)
        );
        assert!(
            composed
                .required_capabilities
                .contains(&BaseTypeCapability::Memory)
        );
    }

    #[test]
    fn identity_manifest_compose_intersects_base_types() {
        let m1 = IdentityManifest::new(
            "comp-a",
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
        let m2 = IdentityManifest::new(
            "comp-b",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Meeting,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let composed = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![m1, m2],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap();
        assert_eq!(composed.supported_base_types.len(), 1);
        assert!(composed.supports_base_type(&BaseTypeId::new("local-harness")));
        assert!(!composed.supports_base_type(&BaseTypeId::new("rusty-clawd")));
    }

    #[test]
    fn identity_manifest_compose_deduplicates_prompt_assets() {
        let asset = PromptAssetRef::new("shared-asset", "path/to/asset.md");
        let m1 = IdentityManifest::new(
            "comp-a",
            "1.0",
            vec![asset.clone()],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let m2 = IdentityManifest::new(
            "comp-b",
            "1.0",
            vec![asset],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Meeting,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let composed = IdentityManifest::compose(
            "composite",
            "1.0",
            vec![m1, m2],
            OperatingMode::Engineer,
            test_contract(),
        )
        .unwrap();
        assert_eq!(
            composed.prompt_assets.len(),
            1,
            "duplicate prompt assets should be deduplicated"
        );
    }
}
