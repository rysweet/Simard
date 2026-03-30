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
}

impl Display for OperatingMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Engineer => "engineer",
            Self::Meeting => "meeting",
            Self::Curator => "curator",
            Self::Improvement => "improvement",
            Self::Gym => "gym",
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
