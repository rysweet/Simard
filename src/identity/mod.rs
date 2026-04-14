mod compose;
mod contract;
mod loader;
mod manifest;
mod types;

// Re-export all public items so `crate::identity::X` still works.
pub use contract::ManifestContract;
pub use loader::{BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader};
pub use manifest::{IdentityManifest, compose_with_precedence};
pub use types::{MemoryPolicy, OperatingMode};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::{BaseTypeId, capability_set};
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
    fn operating_mode_display_all_variants() {
        assert_eq!(OperatingMode::Engineer.to_string(), "engineer");
        assert_eq!(OperatingMode::Meeting.to_string(), "meeting");
        assert_eq!(OperatingMode::Curator.to_string(), "curator");
        assert_eq!(OperatingMode::Improvement.to_string(), "improvement");
        assert_eq!(OperatingMode::Gym.to_string(), "gym");
        assert_eq!(OperatingMode::Orchestrator.to_string(), "orchestrator");
    }

    #[test]
    fn memory_policy_default_is_valid() {
        MemoryPolicy::default().validate().unwrap();
    }

    #[test]
    fn memory_policy_rejects_project_writes() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_scope: crate::memory::MemoryScope::SessionSummary,
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn identity_manifest_construction_and_base_type_check() {
        let manifest = IdentityManifest::new(
            "test-id",
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
        assert!(!manifest.supports_base_type(&BaseTypeId::new("missing")));
    }

    #[test]
    fn identity_manifest_with_components_rejects_self() {
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
        assert!(manifest.with_components(["parent"]).is_err());
    }

    #[test]
    fn compose_with_precedence_single_manifest() {
        let manifest = IdentityManifest::new(
            "solo",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let resolved = compose_with_precedence(vec![manifest]);
        assert_eq!(resolved.base_types, vec![BaseTypeId::new("local-harness")]);
        assert!(resolved.conflict_log.is_empty());
    }
}
