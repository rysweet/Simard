use std::collections::BTreeSet;

use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::error::{SimardError, SimardResult};
use crate::memory::MemoryScope;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::PromptAssetRef;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum OperatingMode {
    Engineer,
    Meeting,
    Gym,
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
        Ok(Self {
            name: name.into(),
            version: version.into(),
            prompt_assets,
            supported_base_types,
            required_capabilities,
            default_mode,
            memory_policy,
            contract,
        })
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
            other => Err(SimardError::UnknownIdentity {
                requested: other.to_string(),
            }),
        }
    }
}
