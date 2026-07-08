//! Identity write-posture resolution (Simard #3125).
//!
//! [`IdentityPosture`] is the target-scoped write posture extracted from an
//! [`IdentityManifest`]: its write authority, its target repo set, and its
//! seed goals. [`ResolvedPosture`] is the boot-time resolution the daemon
//! threads into the OODA state — and the single place the fail-closed rule
//! lives:
//!
//!   * `None` — no identity is present. This is a DETERMINED state: Simard
//!     herself runs read-write and unchanged (AC1).
//!   * `Undetermined` — an identity IS present but its posture could not be
//!     resolved (load / parse / threading gap). This fails CLOSED to
//!     `ReadOnly` so a mis-wired observer can never spawn engineers (AC5).
//!   * `Identity(p)` — a resolved identity posture; use its declared authority.

use super::{IdentityManifest, SeedGoal, WriteAuthority};

/// The target-scoped write posture of a single resolved identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityPosture {
    pub write_authority: WriteAuthority,
    pub targets: Vec<String>,
    pub seed_goals: Vec<SeedGoal>,
}

impl IdentityPosture {
    /// Read the posture (authority + targets + seed goals) off a manifest.
    pub fn from_manifest(manifest: &IdentityManifest) -> Self {
        Self {
            write_authority: manifest.write_authority,
            targets: manifest.targets.clone(),
            seed_goals: manifest.seed_goals.clone(),
        }
    }
}

/// Boot-time resolution of the active identity's write posture.
///
/// The fail-closed contract (no fallbacks / no silent degradation) lives in
/// [`ResolvedPosture::write_authority`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedPosture {
    /// No identity present — Simard's own read-write default (AC1).
    None,
    /// An identity is present but its posture is unresolved — fail CLOSED (AC5).
    Undetermined,
    /// A resolved identity posture.
    Identity(IdentityPosture),
}

impl ResolvedPosture {
    /// The effective write authority for this resolution.
    ///
    /// `None` ⇒ `ReadWrite` (Simard unchanged); `Undetermined` ⇒ `ReadOnly`
    /// (fail closed — never spawn on an unresolved posture); `Identity` ⇒ the
    /// identity's declared authority.
    pub fn write_authority(&self) -> WriteAuthority {
        match self {
            Self::None => WriteAuthority::ReadWrite,
            Self::Undetermined => WriteAuthority::ReadOnly,
            Self::Identity(posture) => posture.write_authority,
        }
    }

    /// The target repo set for this resolution (empty for `None` /
    /// `Undetermined`).
    pub fn targets(&self) -> &[String] {
        match self {
            Self::None | Self::Undetermined => &[],
            Self::Identity(posture) => &posture.targets,
        }
    }

    /// The identity's seed goals (empty for `None` / `Undetermined`).
    pub fn seed_goals(&self) -> &[SeedGoal] {
        match self {
            Self::None | Self::Undetermined => &[],
            Self::Identity(posture) => &posture.seed_goals,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_resolves_to_read_write() {
        assert_eq!(
            ResolvedPosture::None.write_authority(),
            WriteAuthority::ReadWrite
        );
        assert!(ResolvedPosture::None.targets().is_empty());
        assert!(ResolvedPosture::None.seed_goals().is_empty());
    }

    #[test]
    fn undetermined_fails_closed_to_read_only() {
        assert_eq!(
            ResolvedPosture::Undetermined.write_authority(),
            WriteAuthority::ReadOnly
        );
        assert!(ResolvedPosture::Undetermined.targets().is_empty());
        assert!(ResolvedPosture::Undetermined.seed_goals().is_empty());
    }

    #[test]
    fn identity_uses_declared_authority_and_scope() {
        let posture = IdentityPosture {
            write_authority: WriteAuthority::ReadOnly,
            targets: vec!["hyenas/repo-a".to_string()],
            seed_goals: vec![SeedGoal {
                priority: 80,
                title: "Observe branch hygiene".to_string(),
                description: "OBSERVE ONLY".to_string(),
                repo: Some("hyenas/repo-a".to_string()),
            }],
        };
        let resolved = ResolvedPosture::Identity(posture);
        assert_eq!(resolved.write_authority(), WriteAuthority::ReadOnly);
        assert_eq!(resolved.targets(), ["hyenas/repo-a".to_string()]);
        assert_eq!(resolved.seed_goals().len(), 1);
    }
}
