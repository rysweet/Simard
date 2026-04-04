use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::error::{SimardError, SimardResult};
use crate::memory::MemoryScope;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::PromptAssetRef;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatingMode {
    Engineer,
    Meeting,
    Curator,
    Improvement,
    Gym,
    Orchestrator,
}

impl Display for OperatingMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Engineer => "engineer",
            Self::Meeting => "meeting",
            Self::Curator => "curator",
            Self::Improvement => "improvement",
            Self::Gym => "gym",
            Self::Orchestrator => "orchestrator",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryPolicy {
    pub allow_project_writes: bool,
    pub summary_scope: MemoryScope,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            allow_project_writes: false,
            summary_scope: MemoryScope::SessionSummary,
        }
    }
}

impl MemoryPolicy {
    pub fn validate(&self) -> SimardResult<()> {
        if self.allow_project_writes {
            return Err(SimardError::UnsupportedMemoryPolicy {
                field: "memory_policy.allow_project_writes".to_string(),
                reason: "v1 only supports read-only project boundaries".to_string(),
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestContract {
    pub entrypoint: String,
    pub composition: String,
    pub precedence: Vec<String>,
    pub provenance: Provenance,
    pub freshness: Freshness,
}

impl ManifestContract {
    pub fn new(
        entrypoint: impl Into<String>,
        composition: impl Into<String>,
        precedence: Vec<String>,
        provenance: Provenance,
        freshness: Freshness,
    ) -> SimardResult<Self> {
        let entrypoint = required_entrypoint(entrypoint.into())?;
        let composition = required_composition(composition.into())?;
        if precedence.is_empty() {
            return Err(SimardError::InvalidManifestContract {
                field: "precedence".to_string(),
                reason: "at least one precedence value is required".to_string(),
            });
        }
        let mut seen_precedence = BTreeSet::new();
        let precedence = precedence
            .into_iter()
            .map(|value| {
                let value = required_contract_field("precedence", value)?;
                if !value.contains(':') {
                    return Err(SimardError::InvalidManifestContract {
                        field: "precedence".to_string(),
                        reason: "precedence entries must look like 'key:value'".to_string(),
                    });
                }
                if !seen_precedence.insert(value.clone()) {
                    return Err(SimardError::InvalidManifestContract {
                        field: "precedence".to_string(),
                        reason: format!("duplicate precedence value '{value}'"),
                    });
                }
                Ok(value)
            })
            .collect::<SimardResult<Vec<_>>>()?;
        let provenance_source = required_provenance_source(required_contract_field(
            "provenance.source",
            provenance.source,
        )?)?;
        let provenance_locator = required_contract_field("provenance.locator", provenance.locator)?;

        Ok(Self {
            entrypoint,
            composition,
            precedence,
            provenance: Provenance::new(provenance_source, provenance_locator),
            freshness,
        })
    }

    pub fn with_freshness(&self, freshness: Freshness) -> Self {
        Self {
            entrypoint: self.entrypoint.clone(),
            composition: self.composition.clone(),
            precedence: self.precedence.clone(),
            provenance: self.provenance.clone(),
            freshness,
        }
    }
}

fn required_contract_field(field: &str, value: String) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidManifestContract {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

fn required_entrypoint(value: String) -> SimardResult<String> {
    let entrypoint = required_contract_field("entrypoint", value)?;
    if !entrypoint.contains("::") {
        return Err(SimardError::InvalidManifestContract {
            field: "entrypoint".to_string(),
            reason: "expected a Rust-style module::function path".to_string(),
        });
    }
    if entrypoint == "inline-manifest" {
        return Err(SimardError::InvalidManifestContract {
            field: "entrypoint".to_string(),
            reason: "placeholder entrypoints are not allowed".to_string(),
        });
    }
    Ok(entrypoint)
}

fn required_composition(value: String) -> SimardResult<String> {
    let composition = required_contract_field("composition", value)?;
    if !composition.contains("->") {
        return Err(SimardError::InvalidManifestContract {
            field: "composition".to_string(),
            reason: "expected a 'component -> component' composition chain".to_string(),
        });
    }
    Ok(composition)
}

fn required_provenance_source(value: String) -> SimardResult<String> {
    if value == "inline" {
        return Err(SimardError::InvalidManifestContract {
            field: "provenance.source".to_string(),
            reason: "placeholder provenance sources are not allowed".to_string(),
        });
    }
    Ok(value)
}

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

        let mut prompt_assets = Vec::new();
        let mut seen_prompt_assets = BTreeSet::new();
        for component in &components {
            for asset in &component.prompt_assets {
                let key = format!("{}::{}", asset.id, asset.relative_path.display());
                if seen_prompt_assets.insert(key) {
                    prompt_assets.push(asset.clone());
                }
            }
        }

        let mut supported_base_types = components[0].supported_base_types.clone();
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
    fn operating_mode_display_covers_all_variants() {
        assert_eq!(OperatingMode::Engineer.to_string(), "engineer");
        assert_eq!(OperatingMode::Meeting.to_string(), "meeting");
        assert_eq!(OperatingMode::Curator.to_string(), "curator");
        assert_eq!(OperatingMode::Improvement.to_string(), "improvement");
        assert_eq!(OperatingMode::Gym.to_string(), "gym");
        assert_eq!(OperatingMode::Orchestrator.to_string(), "orchestrator");
    }

    #[test]
    fn default_memory_policy_validates_successfully() {
        MemoryPolicy::default().validate().unwrap();
    }

    #[test]
    fn memory_policy_rejects_project_writes() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_scope: MemoryScope::SessionSummary,
        };
        let err = policy.validate().unwrap_err();
        assert!(matches!(err, SimardError::UnsupportedMemoryPolicy { .. }));
    }

    #[test]
    fn manifest_contract_requires_rust_style_entrypoint() {
        let err = ManifestContract::new(
            "no-colons",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "entrypoint"
        ));
    }

    #[test]
    fn manifest_contract_rejects_placeholder_entrypoint() {
        let err = ManifestContract::new(
            "inline-manifest",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_requires_composition_chain() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "no arrow",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "composition"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_precedence() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec![],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "precedence"
        ));
    }

    #[test]
    fn manifest_contract_rejects_duplicate_precedence() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string(), "key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_inline_provenance() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("inline", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "provenance.source"
        ));
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

    // --- OperatingMode serde ---

    #[test]
    fn operating_mode_serializes_to_kebab_case() {
        let json = serde_json::to_string(&OperatingMode::Orchestrator).unwrap();
        assert_eq!(json, "\"orchestrator\"");
        let json = serde_json::to_string(&OperatingMode::Improvement).unwrap();
        assert_eq!(json, "\"improvement\"");
    }

    #[test]
    fn operating_mode_deserializes_from_kebab_case() {
        let mode: OperatingMode = serde_json::from_str("\"engineer\"").unwrap();
        assert_eq!(mode, OperatingMode::Engineer);
        let mode: OperatingMode = serde_json::from_str("\"gym\"").unwrap();
        assert_eq!(mode, OperatingMode::Gym);
    }

    #[test]
    fn operating_mode_roundtrips_through_serde() {
        let modes = [
            OperatingMode::Engineer,
            OperatingMode::Meeting,
            OperatingMode::Curator,
            OperatingMode::Improvement,
            OperatingMode::Gym,
            OperatingMode::Orchestrator,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let back: OperatingMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn operating_mode_ord_is_consistent() {
        assert!(OperatingMode::Engineer < OperatingMode::Meeting);
        assert!(OperatingMode::Gym < OperatingMode::Orchestrator);
    }

    // --- MemoryPolicy ---

    #[test]
    fn memory_policy_default_values() {
        let policy = MemoryPolicy::default();
        assert!(!policy.allow_project_writes);
        assert_eq!(policy.summary_scope, MemoryScope::SessionSummary);
    }

    #[test]
    fn memory_policy_project_writes_error_message() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_scope: MemoryScope::SessionSummary,
        };
        let err = policy.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("read-only project boundaries"));
    }

    // --- ManifestContract ---

    #[test]
    fn manifest_contract_valid_construction() {
        let contract = ManifestContract::new(
            "module::function",
            "a -> b",
            vec!["identity:simard".to_string(), "base-type:local".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap();
        assert_eq!(contract.entrypoint, "module::function");
        assert_eq!(contract.composition, "a -> b");
        assert_eq!(contract.precedence.len(), 2);
    }

    #[test]
    fn manifest_contract_rejects_empty_entrypoint() {
        let err = ManifestContract::new(
            "",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "entrypoint"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_composition() {
        let err = ManifestContract::new(
            "test::entry",
            "",
            vec!["key:value".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "composition"
        ));
    }

    #[test]
    fn manifest_contract_rejects_precedence_without_colon() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["no-colon-here".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "precedence"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_precedence_value() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["  ".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_empty_provenance_source() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_empty_provenance_locator() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("src", ""),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_with_freshness_preserves_other_fields() {
        let contract = test_contract();
        let new_freshness = Freshness::now().unwrap();
        let updated = contract.with_freshness(new_freshness);
        assert_eq!(updated.entrypoint, contract.entrypoint);
        assert_eq!(updated.composition, contract.composition);
        assert_eq!(updated.precedence, contract.precedence);
        assert_eq!(updated.freshness, new_freshness);
    }

    // --- IdentityManifest ---

    #[test]
    fn identity_manifest_new_rejects_project_writes_policy() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_scope: MemoryScope::SessionSummary,
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
