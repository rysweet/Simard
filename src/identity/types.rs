use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::memory::MemoryScope;

/// Behavioral mode that determines which prompt assets, memory policies,
/// and session configurations Simard loads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatingMode {
    /// Primary engineering loop — code generation, review, debugging.
    Engineer,
    /// Multi-agent meeting facilitation with agenda and consensus.
    Meeting,
    /// Memory curation and knowledge graph maintenance.
    Curator,
    /// Self-improvement cycle — assess, plan, apply, verify.
    Improvement,
    /// Evaluation gym — progressive scenario execution (L1–L12).
    Gym,
    /// Workflow orchestration — recipe routing and workstream dispatch.
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

impl FromStr for OperatingMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "engineer" => Ok(Self::Engineer),
            "meeting" => Ok(Self::Meeting),
            "curator" => Ok(Self::Curator),
            "improvement" => Ok(Self::Improvement),
            "gym" => Ok(Self::Gym),
            "orchestrator" => Ok(Self::Orchestrator),
            other => Err(format!("unknown operating mode: '{other}'")),
        }
    }
}

/// Write posture of an identity (Simard #3125).
///
/// A read-only OBSERVER identity (e.g. Crocutus watching the hyenas repos) is
/// authorized to observe and propose goals but NEVER to spawn write-bearing
/// engineers. `ReadWrite` is the default so Simard herself — and any identity
/// that does not declare a posture — is behaviorally unchanged.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum WriteAuthority {
    /// OBSERVE ONLY: may record observations and propose goals, but the Act
    /// phase must not dispatch engineers or write to any target repo.
    ReadOnly,
    /// Full authority — the Simard-unchanged default. May spawn engineers and
    /// open PRs against the goal's target repo.
    #[default]
    ReadWrite,
}

impl Display for WriteAuthority {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        };
        f.write_str(label)
    }
}

impl WriteAuthority {
    /// `true` only for [`WriteAuthority::ReadWrite`].
    ///
    /// The deterministic spawn rail matches on this (deny-by-default): it
    /// authorizes engineer dispatch ONLY for the proven read-write case, so any
    /// read-only — or, should a future variant appear, any non-read-write —
    /// posture fails closed and cannot spawn a write-bearing engineer.
    pub fn may_dispatch_engineers(&self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    /// `true` for [`WriteAuthority::ReadOnly`] — an observe-only posture.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

/// A single identity-declared seed goal (Simard #3125).
///
/// When an identity declares seed goals they OVERRIDE Simard's baked-in
/// `DEFAULT_SEED_GOALS`. `repo` is the target-repo slug the goal is scoped to
/// (e.g. `"hyenas/repo-a"`); for a read-only identity every seed goal MUST be
/// scoped to a repo within the identity's target set (enforced by
/// [`IdentityManifest::with_posture`]) so a goal can never silently escape to
/// `rysweet/Simard`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedGoal {
    pub priority: u32,
    pub title: String,
    pub description: String,
    pub repo: Option<String>,
}

/// Controls what a session is allowed to write to long-term memory.
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

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- WriteAuthority (Simard #3125) ---

    #[test]
    fn write_authority_default_is_read_write() {
        assert_eq!(WriteAuthority::default(), WriteAuthority::ReadWrite);
    }

    #[test]
    fn write_authority_display_is_kebab() {
        assert_eq!(WriteAuthority::ReadOnly.to_string(), "read-only");
        assert_eq!(WriteAuthority::ReadWrite.to_string(), "read-write");
    }

    #[test]
    fn write_authority_may_dispatch_only_read_write() {
        assert!(WriteAuthority::ReadWrite.may_dispatch_engineers());
        assert!(!WriteAuthority::ReadOnly.may_dispatch_engineers());
    }

    #[test]
    fn write_authority_is_read_only_only_read_only() {
        assert!(WriteAuthority::ReadOnly.is_read_only());
        assert!(!WriteAuthority::ReadWrite.is_read_only());
    }

    #[test]
    fn write_authority_serde_kebab_roundtrip() {
        assert_eq!(
            serde_json::to_string(&WriteAuthority::ReadOnly).unwrap(),
            "\"read-only\""
        );
        let back: WriteAuthority = serde_json::from_str("\"read-write\"").unwrap();
        assert_eq!(back, WriteAuthority::ReadWrite);
    }

    #[test]
    fn seed_goal_holds_target_scope() {
        let g = SeedGoal {
            priority: 80,
            title: "Observe branch hygiene".to_string(),
            description: "OBSERVE ONLY".to_string(),
            repo: Some("hyenas/repo-a".to_string()),
        };
        assert_eq!(g.repo.as_deref(), Some("hyenas/repo-a"));
        assert_eq!(g.priority, 80);
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

    #[test]
    fn operating_mode_fromstr_valid() {
        assert_eq!(
            "engineer".parse::<OperatingMode>().unwrap(),
            OperatingMode::Engineer
        );
        assert_eq!(
            "meeting".parse::<OperatingMode>().unwrap(),
            OperatingMode::Meeting
        );
        assert_eq!(
            "curator".parse::<OperatingMode>().unwrap(),
            OperatingMode::Curator
        );
        assert_eq!(
            "improvement".parse::<OperatingMode>().unwrap(),
            OperatingMode::Improvement
        );
        assert_eq!("gym".parse::<OperatingMode>().unwrap(), OperatingMode::Gym);
        assert_eq!(
            "orchestrator".parse::<OperatingMode>().unwrap(),
            OperatingMode::Orchestrator
        );
    }

    #[test]
    fn operating_mode_fromstr_invalid() {
        assert!("unknown".parse::<OperatingMode>().is_err());
        assert!("Engineer".parse::<OperatingMode>().is_err());
        assert!("".parse::<OperatingMode>().is_err());
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
}
