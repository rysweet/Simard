use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::memory::CognitiveMemoryType;

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

/// Controls what a session is allowed to write to long-term memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryPolicy {
    pub allow_project_writes: bool,
    pub summary_memory_type: CognitiveMemoryType,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            allow_project_writes: false,
            summary_memory_type: CognitiveMemoryType::Episodic,
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

    #[test]
    fn memory_policy_rejects_project_writes() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_memory_type: CognitiveMemoryType::Episodic,
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

    // --- MemoryPolicy ---

    #[test]
    fn memory_policy_default_values() {
        let policy = MemoryPolicy::default();
        assert!(!policy.allow_project_writes);
        assert_eq!(policy.summary_memory_type, CognitiveMemoryType::Episodic);
    }

    #[test]
    fn memory_policy_project_writes_error_message() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_memory_type: CognitiveMemoryType::Episodic,
        };
        let err = policy.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("read-only project boundaries"));
    }
}
