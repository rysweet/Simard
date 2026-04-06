use std::fmt;

use serde::{Deserialize, Serialize};

use crate::session::{SessionId, SessionPhase};

pub(crate) const MEMORY_STORE_NAME: &str = "memory";

/// The six cognitive memory types from the cognitive psychology model.
///
/// This replaces the old ad-hoc `MemoryScope` with the scientifically-grounded
/// memory taxonomy used by `amplihack-memory-lib`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CognitiveMemoryType {
    /// Transient sensory buffer — raw observations with short TTL.
    Sensory,
    /// Active task context — ephemeral slots bound to the current task.
    Working,
    /// Event records — session summaries, experiences, temporal sequences.
    Episodic,
    /// Factual knowledge — decisions, project context, domain facts.
    Semantic,
    /// Skill and process knowledge — benchmarks, procedures, how-to.
    Procedural,
    /// Future intentions — goals, triggers, deferred plans.
    Prospective,
}

impl fmt::Display for CognitiveMemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sensory => write!(f, "sensory"),
            Self::Working => write!(f, "working"),
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
            Self::Prospective => write!(f, "prospective"),
        }
    }
}
/// Accept both new kebab-case names and legacy MemoryScope variant names.
impl<'de> Deserialize<'de> for CognitiveMemoryType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CognitiveMemoryTypeVisitor;

        impl<'de> serde::de::Visitor<'de> for CognitiveMemoryTypeVisitor {
            type Value = CognitiveMemoryType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a cognitive memory type or legacy scope name")
            }

            fn visit_str<E>(self, value: &str) -> Result<CognitiveMemoryType, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "sensory" => Ok(CognitiveMemoryType::Sensory),
                    "working" => Ok(CognitiveMemoryType::Working),
                    "episodic" => Ok(CognitiveMemoryType::Episodic),
                    "semantic" => Ok(CognitiveMemoryType::Semantic),
                    "procedural" => Ok(CognitiveMemoryType::Procedural),
                    "prospective" => Ok(CognitiveMemoryType::Prospective),
                    // Legacy MemoryScope variant names
                    "session-scratch" => Ok(CognitiveMemoryType::Working),
                    "session-summary" => Ok(CognitiveMemoryType::Episodic),
                    "decision" => Ok(CognitiveMemoryType::Semantic),
                    "project" => Ok(CognitiveMemoryType::Semantic),
                    "benchmark" => Ok(CognitiveMemoryType::Procedural),
                    _ => Err(serde::de::Error::unknown_variant(
                        value,
                        &[
                            "sensory",
                            "working",
                            "episodic",
                            "semantic",
                            "procedural",
                            "prospective",
                            "session-scratch",
                            "session-summary",
                            "decision",
                            "project",
                            "benchmark",
                        ],
                    )),
                }
            }
        }

        deserializer.deserialize_str(CognitiveMemoryTypeVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    #[serde(alias = "scope")]
    pub memory_type: CognitiveMemoryType,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
}
