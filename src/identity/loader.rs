use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::error::{SimardError, SimardResult};
use crate::prompt_assets::PromptAssetRef;

use super::{IdentityManifest, ManifestContract, MemoryPolicy, OperatingMode};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityLoadRequest {
    pub identity: String,
    pub package_version: String,
    pub contract: ManifestContract,
}

impl IdentityLoadRequest {
    pub fn new(
        identity: impl Into<String>,
        package_version: impl Into<String>,
        contract: ManifestContract,
    ) -> Self {
        Self {
            identity: identity.into(),
            package_version: package_version.into(),
            contract,
        }
    }
}

pub trait IdentityLoader {
    fn load(&self, request: &IdentityLoadRequest) -> SimardResult<IdentityManifest>;
}

#[derive(Default)]
pub struct BuiltinIdentityLoader;

impl IdentityLoader for BuiltinIdentityLoader {
    fn load(&self, request: &IdentityLoadRequest) -> SimardResult<IdentityManifest> {
        match request.identity.as_str() {
            "simard-engineer" => IdentityManifest::new(
                "simard-engineer",
                request.package_version.clone(),
                vec![PromptAssetRef::new(
                    "engineer-system",
                    "simard/engineer_system.md",
                )],
                vec![
                    BaseTypeId::new("local-harness"),
                    BaseTypeId::new("terminal-shell"),
                    BaseTypeId::new("rusty-clawd"),
                    BaseTypeId::new("copilot-sdk"),
                    BaseTypeId::new("claude-agent-sdk"),
                    BaseTypeId::new("ms-agent-framework"),
                ],
                capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                OperatingMode::Engineer,
                MemoryPolicy::default(),
                request.contract.clone(),
            ),
            "simard-meeting" => IdentityManifest::new(
                "simard-meeting",
                request.package_version.clone(),
                vec![PromptAssetRef::new(
                    "meeting-system",
                    "simard/meeting_system.md",
                )],
                vec![
                    BaseTypeId::new("local-harness"),
                    BaseTypeId::new("rusty-clawd"),
                    BaseTypeId::new("copilot-sdk"),
                    BaseTypeId::new("claude-agent-sdk"),
                    BaseTypeId::new("ms-agent-framework"),
                ],
                capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                OperatingMode::Meeting,
                MemoryPolicy::default(),
                request.contract.clone(),
            ),
            "simard-gym" => IdentityManifest::new(
                "simard-gym",
                request.package_version.clone(),
                vec![PromptAssetRef::new("gym-system", "simard/gym_system.md")],
                vec![
                    BaseTypeId::new("local-harness"),
                    BaseTypeId::new("rusty-clawd"),
                    BaseTypeId::new("copilot-sdk"),
                    BaseTypeId::new("claude-agent-sdk"),
                    BaseTypeId::new("ms-agent-framework"),
                ],
                capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                OperatingMode::Gym,
                MemoryPolicy::default(),
                request.contract.clone(),
            ),
            "simard-goal-curator" => IdentityManifest::new(
                "simard-goal-curator",
                request.package_version.clone(),
                vec![PromptAssetRef::new(
                    "goal-curator-system",
                    "simard/goal_curator_system.md",
                )],
                vec![
                    BaseTypeId::new("local-harness"),
                    BaseTypeId::new("rusty-clawd"),
                    BaseTypeId::new("copilot-sdk"),
                    BaseTypeId::new("claude-agent-sdk"),
                    BaseTypeId::new("ms-agent-framework"),
                ],
                capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                OperatingMode::Curator,
                MemoryPolicy::default(),
                request.contract.clone(),
            ),
            "simard-improvement-curator" => IdentityManifest::new(
                "simard-improvement-curator",
                request.package_version.clone(),
                vec![PromptAssetRef::new(
                    "improvement-curator-system",
                    "simard/improvement_curator_system.md",
                )],
                vec![
                    BaseTypeId::new("local-harness"),
                    BaseTypeId::new("rusty-clawd"),
                    BaseTypeId::new("copilot-sdk"),
                    BaseTypeId::new("claude-agent-sdk"),
                    BaseTypeId::new("ms-agent-framework"),
                ],
                capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                OperatingMode::Improvement,
                MemoryPolicy::default(),
                request.contract.clone(),
            ),
            "simard-composite-engineer" => IdentityManifest::compose(
                "simard-composite-engineer",
                request.package_version.clone(),
                vec![
                    self.load(&IdentityLoadRequest::new(
                        "simard-engineer",
                        request.package_version.clone(),
                        request.contract.clone(),
                    ))?,
                    self.load(&IdentityLoadRequest::new(
                        "simard-meeting",
                        request.package_version.clone(),
                        request.contract.clone(),
                    ))?,
                    self.load(&IdentityLoadRequest::new(
                        "simard-gym",
                        request.package_version.clone(),
                        request.contract.clone(),
                    ))?,
                    self.load(&IdentityLoadRequest::new(
                        "simard-goal-curator",
                        request.package_version.clone(),
                        request.contract.clone(),
                    ))?,
                    self.load(&IdentityLoadRequest::new(
                        "simard-improvement-curator",
                        request.package_version.clone(),
                        request.contract.clone(),
                    ))?,
                ],
                OperatingMode::Engineer,
                request.contract.clone(),
            ),
            other => Err(SimardError::UnknownIdentity {
                requested: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn builtin_loader_loads_engineer_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-engineer",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-engineer");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    }

    #[test]
    fn builtin_loader_rejects_unknown_identity() {
        let loader = BuiltinIdentityLoader;
        let err = loader
            .load(&IdentityLoadRequest::new(
                "unknown",
                "0.1.0",
                test_contract(),
            ))
            .unwrap_err();
        assert!(matches!(err, SimardError::UnknownIdentity { .. }));
    }

    // --- IdentityLoadRequest ---

    #[test]
    fn identity_load_request_construction() {
        let req = IdentityLoadRequest::new("test-id", "1.0.0", test_contract());
        assert_eq!(req.identity, "test-id");
        assert_eq!(req.package_version, "1.0.0");
    }

    // --- BuiltinIdentityLoader: all identities ---

    #[test]
    fn builtin_loader_loads_meeting_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-meeting",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-meeting");
        assert_eq!(manifest.default_mode, OperatingMode::Meeting);
    }

    #[test]
    fn builtin_loader_loads_gym_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-gym",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-gym");
        assert_eq!(manifest.default_mode, OperatingMode::Gym);
    }

    #[test]
    fn builtin_loader_loads_goal_curator_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-goal-curator",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-goal-curator");
        assert_eq!(manifest.default_mode, OperatingMode::Curator);
    }

    #[test]
    fn builtin_loader_loads_improvement_curator_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-improvement-curator",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-improvement-curator");
        assert_eq!(manifest.default_mode, OperatingMode::Improvement);
    }

    #[test]
    fn builtin_loader_loads_composite_engineer_identity() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-composite-engineer",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert_eq!(manifest.name, "simard-composite-engineer");
        assert_eq!(manifest.default_mode, OperatingMode::Engineer);
        assert!(
            !manifest.components.is_empty(),
            "composite should have components"
        );
    }

    #[test]
    fn builtin_loader_all_identities_share_local_harness() {
        let loader = BuiltinIdentityLoader;
        let names = [
            "simard-engineer",
            "simard-meeting",
            "simard-gym",
            "simard-goal-curator",
            "simard-improvement-curator",
        ];
        for name in names {
            let manifest = loader
                .load(&IdentityLoadRequest::new(name, "0.1.0", test_contract()))
                .unwrap();
            assert!(
                manifest.supports_base_type(&BaseTypeId::new("local-harness")),
                "{name} should support local-harness"
            );
        }
    }

    #[test]
    fn builtin_loader_engineer_has_prompt_assets() {
        let loader = BuiltinIdentityLoader;
        let manifest = loader
            .load(&IdentityLoadRequest::new(
                "simard-engineer",
                "0.1.0",
                test_contract(),
            ))
            .unwrap();
        assert!(!manifest.prompt_assets.is_empty());
        assert_eq!(manifest.prompt_assets[0].id.as_str(), "engineer-system");
    }
}
