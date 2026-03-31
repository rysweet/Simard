//! Role catalog for agent composition.
//!
//! Defines the standard agent roles used in Simard's multi-agent
//! composition model (Pillar 9: Composition Outlives Topology).
//! Each role maps to a pre-configured `IdentityManifest` template
//! that can be instantiated for subordinate agents.

use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::error::SimardResult;
use crate::identity::{IdentityManifest, ManifestContract, MemoryPolicy, OperatingMode};
use crate::memory::MemoryScope;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::PromptAssetRef;

/// Standard roles that subordinate agents may assume within a composition.
///
/// Each role carries an implied operating mode, capabilities, and prompt
/// asset set. The role catalog is intentionally small: adding a role
/// requires explicit justification against the product architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AgentRole {
    /// Writes code, runs tests, mutates the repository.
    Engineer,
    /// Reviews artifacts, produces improvement proposals.
    Reviewer,
    /// Executes benchmark scenarios and persists results.
    GymRunner,
    /// Coordinates meetings and curates goal updates.
    Facilitator,
}

impl Display for AgentRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Engineer => "engineer",
            Self::Reviewer => "reviewer",
            Self::GymRunner => "gym-runner",
            Self::Facilitator => "facilitator",
        };
        f.write_str(label)
    }
}

impl AgentRole {
    /// The operating mode this role implies.
    pub fn operating_mode(self) -> OperatingMode {
        match self {
            Self::Engineer => OperatingMode::Engineer,
            Self::Reviewer => OperatingMode::Engineer,
            Self::GymRunner => OperatingMode::Gym,
            Self::Facilitator => OperatingMode::Meeting,
        }
    }

    /// The minimum capabilities required for this role.
    pub fn required_capabilities(self) -> BTreeSet<BaseTypeCapability> {
        match self {
            Self::Engineer => capability_set([
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
                BaseTypeCapability::Memory,
                BaseTypeCapability::Evidence,
                BaseTypeCapability::Reflection,
            ]),
            Self::Reviewer => capability_set([
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
                BaseTypeCapability::Memory,
                BaseTypeCapability::Reflection,
            ]),
            Self::GymRunner => capability_set([
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
                BaseTypeCapability::Memory,
                BaseTypeCapability::Evidence,
                BaseTypeCapability::Reflection,
            ]),
            Self::Facilitator => capability_set([
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
                BaseTypeCapability::Memory,
                BaseTypeCapability::Evidence,
                BaseTypeCapability::Reflection,
            ]),
        }
    }

    /// Supported base types for this role.
    fn supported_base_types(self) -> Vec<BaseTypeId> {
        vec![
            BaseTypeId::new("local-harness"),
            BaseTypeId::new("rusty-clawd"),
            BaseTypeId::new("copilot-sdk"),
        ]
    }

    /// Prompt assets for this role.
    fn prompt_assets(self) -> Vec<PromptAssetRef> {
        match self {
            Self::Engineer => vec![PromptAssetRef::new(
                "engineer-system",
                "simard/engineer_system.md",
            )],
            Self::Reviewer => vec![PromptAssetRef::new(
                "reviewer-system",
                "simard/reviewer_system.md",
            )],
            Self::GymRunner => vec![PromptAssetRef::new("gym-system", "simard/gym_system.md")],
            Self::Facilitator => vec![PromptAssetRef::new(
                "meeting-system",
                "simard/meeting_system.md",
            )],
        }
    }

    /// Identity name prefix for subordinates of this role.
    pub fn identity_name(self) -> &'static str {
        match self {
            Self::Engineer => "simard-sub-engineer",
            Self::Reviewer => "simard-sub-reviewer",
            Self::GymRunner => "simard-sub-gym",
            Self::Facilitator => "simard-sub-facilitator",
        }
    }
}

/// Select the most appropriate role for a given objective string.
///
/// Uses keyword heuristics to match objectives to roles. Falls back to
/// `Engineer` when no specific keywords match -- engineering is the
/// default subordinate role per the product architecture.
pub fn role_for_objective(objective: &str) -> AgentRole {
    let lower = objective.to_lowercase();

    if lower.contains("review") || lower.contains("audit") || lower.contains("inspect") {
        return AgentRole::Reviewer;
    }
    if lower.contains("benchmark") || lower.contains("gym") || lower.contains("performance") {
        return AgentRole::GymRunner;
    }
    if lower.contains("meeting") || lower.contains("facilitate") || lower.contains("coordinate") {
        return AgentRole::Facilitator;
    }

    AgentRole::Engineer
}

/// Construct an `IdentityManifest` for the given role.
///
/// The manifest uses role-specific defaults and a synthetic provenance
/// indicating it was generated by the composition system. The returned
/// manifest is validated before return.
pub fn identity_for_role(role: AgentRole) -> SimardResult<IdentityManifest> {
    let mode = role.operating_mode();
    let freshness = Freshness::now()?;

    let contract = ManifestContract::new(
        "simard::agent_roles::identity_for_role",
        "role-catalog -> identity-manifest -> subordinate-runtime",
        vec![format!("composition:role={role}")],
        Provenance::new("composition", format!("agent_roles::{role}")),
        freshness,
    )?;

    IdentityManifest::new(
        role.identity_name(),
        env!("CARGO_PKG_VERSION"),
        role.prompt_assets(),
        role.supported_base_types(),
        role.required_capabilities(),
        mode,
        MemoryPolicy {
            allow_project_writes: false,
            summary_scope: match role {
                AgentRole::GymRunner => MemoryScope::Benchmark,
                _ => MemoryScope::SessionSummary,
            },
        },
        contract,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_for_objective_selects_engineer_by_default() {
        assert_eq!(
            role_for_objective("implement the parser"),
            AgentRole::Engineer
        );
    }

    #[test]
    fn role_for_objective_selects_reviewer() {
        assert_eq!(role_for_objective("review the PR"), AgentRole::Reviewer);
        assert_eq!(
            role_for_objective("audit the codebase"),
            AgentRole::Reviewer
        );
    }

    #[test]
    fn role_for_objective_selects_gym_runner() {
        assert_eq!(
            role_for_objective("run benchmark suite"),
            AgentRole::GymRunner
        );
        assert_eq!(
            role_for_objective("gym performance test"),
            AgentRole::GymRunner
        );
    }

    #[test]
    fn role_for_objective_selects_facilitator() {
        assert_eq!(
            role_for_objective("facilitate the meeting"),
            AgentRole::Facilitator
        );
        assert_eq!(
            role_for_objective("coordinate team sync"),
            AgentRole::Facilitator
        );
    }

    #[test]
    fn identity_for_role_produces_valid_manifests() {
        for role in [AgentRole::Engineer, AgentRole::Reviewer] {
            let manifest = identity_for_role(role).expect("manifest should be valid");
            assert_eq!(manifest.default_mode, role.operating_mode());
            assert!(manifest.name.starts_with("simard-sub-"));
        }
    }

    #[test]
    fn role_display_is_lowercase_kebab() {
        assert_eq!(AgentRole::GymRunner.to_string(), "gym-runner");
        assert_eq!(AgentRole::Engineer.to_string(), "engineer");
    }
}
