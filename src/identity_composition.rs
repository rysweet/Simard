//! Composite identity for multi-agent compositions.
//!
//! Implements Pillar 8 (Identity != Runtime) and Pillar 9 (Composition
//! Outlives Topology) from the product architecture. A `CompositeIdentity`
//! binds a primary agent to zero or more subordinates, each with an
//! assigned role and recursion depth limit.

use std::fmt::{self, Display, Formatter};
use std::sync::OnceLock;

use crate::agent_roles::AgentRole;
use crate::error::{SimardError, SimardResult};
use crate::identity::IdentityManifest;

/// Maximum subordinate nesting depth from the environment, defaulting to
/// unlimited (u32::MAX). External agent tools (Copilot, Claude, etc.) have
/// their own guardrails — Simard should not impose artificial depth limits.
const ENV_MAX_DEPTH: &str = "SIMARD_MAX_SUBORDINATE_DEPTH";
const DEFAULT_MAX_DEPTH: u32 = u32::MAX;

/// A subordinate's identity within a composition.
///
/// Each subordinate has its own `IdentityManifest` (ensuring memory
/// isolation via distinct `agent_name`), an assigned role, and a maximum
/// recursion depth controlling how deep this subordinate may itself
/// compose further subordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubordinateIdentity {
    /// The subordinate's own identity manifest.
    pub manifest: IdentityManifest,
    /// The role this subordinate fills in the composition.
    pub role: AgentRole,
    /// Maximum depth this subordinate may spawn further subordinates.
    /// 0 means it cannot spawn any.
    pub max_depth: u32,
}

impl Display for SubordinateIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}(role={}, depth={})",
            self.manifest.name, self.role, self.max_depth
        )
    }
}

/// A composite identity binding a primary agent to its subordinates.
///
/// The primary agent is the supervisor. Subordinates are spawned and
/// managed through the `agent_supervisor` module. Communication happens
/// through hive-based semantic facts, not direct IPC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositeIdentity {
    /// The supervisor agent's identity.
    pub primary: IdentityManifest,
    /// Subordinate agents in this composition.
    pub subordinates: Vec<SubordinateIdentity>,
}

impl Display for CompositeIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "CompositeIdentity(primary={}", self.primary.name)?;
        if !self.subordinates.is_empty() {
            write!(f, ", subordinates=[")?;
            for (i, sub) in self.subordinates.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{sub}")?;
            }
            write!(f, "]")?;
        }
        write!(f, ")")
    }
}

/// Read the maximum subordinate depth from the environment.
///
/// The value is read once and cached for the lifetime of the process.
/// Returns `SIMARD_MAX_SUBORDINATE_DEPTH` if set and valid, otherwise
/// defaults to unlimited (`u32::MAX`).
pub fn max_subordinate_depth() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var(ENV_MAX_DEPTH)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_MAX_DEPTH)
    })
}

/// Compose a primary identity with a set of subordinate identities.
///
/// Validates that:
/// - No subordinate shares the primary's agent name (memory isolation).
/// - No two subordinates share the same agent name.
/// - No subordinate's `max_depth` exceeds the environment limit.
/// - At least the primary identity is present (subordinates may be empty).
pub fn compose_identity(
    primary: IdentityManifest,
    subordinates: Vec<SubordinateIdentity>,
) -> SimardResult<CompositeIdentity> {
    // Validate no name collisions with primary.
    for sub in &subordinates {
        if sub.manifest.name == primary.name {
            return Err(SimardError::InvalidIdentityComposition {
                identity: primary.name.clone(),
                reason: format!(
                    "subordinate '{}' shares the primary agent name — memory isolation requires distinct names",
                    sub.manifest.name
                ),
            });
        }
    }

    // Validate no duplicate subordinate names.
    let mut seen_names = std::collections::BTreeSet::new();
    for sub in &subordinates {
        if !seen_names.insert(&sub.manifest.name) {
            return Err(SimardError::InvalidIdentityComposition {
                identity: primary.name.clone(),
                reason: format!(
                    "duplicate subordinate name '{}' — each subordinate must have a unique agent name",
                    sub.manifest.name
                ),
            });
        }
    }

    // Log depth warnings but never block composition — external tools enforce
    // their own session guardrails.
    let depth_limit = max_subordinate_depth();
    for sub in &subordinates {
        if depth_limit < u32::MAX && sub.max_depth > depth_limit {
            eprintln!(
                "warning: subordinate '{}' has max_depth {} exceeding env limit {} — proceeding anyway",
                sub.manifest.name, sub.max_depth, depth_limit
            );
        }
    }

    Ok(CompositeIdentity {
        primary,
        subordinates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_roles::identity_for_role;

    fn test_primary() -> IdentityManifest {
        identity_for_role(AgentRole::Engineer).expect("primary manifest should be valid")
    }

    fn test_sub(role: AgentRole, name_suffix: &str, depth: u32) -> SubordinateIdentity {
        let mut manifest = identity_for_role(role).expect("subordinate manifest should be valid");
        manifest.name = format!("{}-{name_suffix}", role.identity_name());
        SubordinateIdentity {
            manifest,
            role,
            max_depth: depth,
        }
    }

    #[test]
    fn compose_empty_subordinates_succeeds() {
        let primary = test_primary();
        let composite =
            compose_identity(primary.clone(), vec![]).expect("empty composition should succeed");
        assert_eq!(composite.primary.name, primary.name);
        assert!(composite.subordinates.is_empty());
    }

    #[test]
    fn compose_with_subordinates_succeeds() {
        let primary = test_primary();
        let sub = test_sub(AgentRole::Reviewer, "1", 1);
        let composite = compose_identity(primary.clone(), vec![sub])
            .expect("single subordinate composition should succeed");
        assert_eq!(composite.subordinates.len(), 1);
        assert_eq!(composite.subordinates[0].role, AgentRole::Reviewer);
    }

    #[test]
    fn compose_rejects_name_collision_with_primary() {
        let primary = test_primary();
        let mut sub = test_sub(AgentRole::Reviewer, "1", 1);
        sub.manifest.name = primary.name.clone();

        let err =
            compose_identity(primary, vec![sub]).expect_err("name collision should be rejected");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn compose_rejects_duplicate_subordinate_names() {
        let primary = test_primary();
        let sub1 = test_sub(AgentRole::Reviewer, "dup", 1);
        let mut sub2 = test_sub(AgentRole::Engineer, "other", 1);
        sub2.manifest.name = sub1.manifest.name.clone();

        let err = compose_identity(primary, vec![sub1, sub2])
            .expect_err("duplicate subordinate names should be rejected");
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn compose_accepts_any_depth() {
        let primary = test_primary();
        let depth_limit = max_subordinate_depth();
        // Even exceeding the env limit should succeed — external tools have guardrails.
        let sub = test_sub(AgentRole::Reviewer, "deep", depth_limit.saturating_add(1));

        let result = compose_identity(primary, vec![sub]);
        assert!(result.is_ok(), "depth should not block composition");
    }

    #[test]
    fn max_subordinate_depth_returns_default_when_unset() {
        // Default is now u32::MAX (unlimited) — external tools have guardrails.
        let depth = max_subordinate_depth();
        assert!(depth >= 1);
    }

    #[test]
    fn display_formats_composite_identity() {
        let primary = test_primary();
        let sub = test_sub(AgentRole::Reviewer, "1", 2);
        let composite = compose_identity(primary, vec![sub]).expect("composition should succeed");
        let display = composite.to_string();
        assert!(display.contains("CompositeIdentity"));
        assert!(display.contains("subordinates="));
    }
}
